use super::{App, Device, Route, egui, theme};
use crate::terminal::{self, Session};
use egui::{Color32, FontFamily, FontId, Key, RichText, TextFormat, text::LayoutJob};
use std::{sync::{Arc,atomic::{AtomicUsize,Ordering}},time::Duration};

const MAX_TABS: usize = 8;
const COPIED_SECONDS: f64 = 2.;

fn copied_badge(ui:&mut egui::Ui,deadline:Option<f64>){
    let now=ui.input(|i|i.time);
    let (fill,ink)=if let Some(until)=deadline.filter(|until|now<*until) {
        ui.ctx().request_repaint_after(Duration::from_secs_f64(until-now));
        let colors=theme::roles(ui.ctx());
        (colors.accent,colors.on_accent)
    } else {(Color32::TRANSPARENT,Color32::TRANSPARENT)};
    egui::Frame::new().fill(fill).corner_radius(12)
        .inner_margin(egui::Margin::symmetric(10,4))
        .show(ui,|ui|ui.add(egui::Label::new(RichText::new("Copied").strong().size(16.).color(ink)).selectable(false)));
}

// Explicit read-only text widget copy commands are available synchronously.
fn observe_copy(ctx:&egui::Context,deadline:&mut Option<f64>,widget:impl FnOnce()){
    let before=ctx.output(|o|o.commands.len());
    widget();
    let copied=ctx.output(|o|o.commands.iter().skip(before)
        .any(|c|matches!(c,egui::OutputCommand::CopyText(text) if !text.is_empty())));
    if copied {
        *deadline=Some(ctx.input(|i|i.time)+COPIED_SECONDS);
        ctx.request_repaint();
    }
}

// The normal terminal's selectable labels have deferred output. This hook
// sees only completed per-pass output. It never calls Context, takes app/UI
// locks, or retains selected text; the UI never looks up/locks this plugin.
struct DeferredCopyOutput(Arc<AtomicUsize>);
const COPY_IDLE:usize=0;
const COPY_EMITTED:usize=1;
const COPY_ARMED_OFFSET:usize=2;

impl egui::Plugin for DeferredCopyOutput {
    fn debug_name(&self)->&'static str {"Terminal selection copy output"}

    fn output_hook(&mut self,output:&mut egui::FullOutput){
        let token=self.0.swap(COPY_IDLE,Ordering::Relaxed);
        if let Some(before)=token.checked_sub(COPY_ARMED_OFFSET)
            && output.platform_output.commands.iter().skip(before)
                .any(|c|matches!(c,egui::OutputCommand::CopyText(s) if !s.is_empty()))
        {
            self.0.store(COPY_EMITTED,Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct DeferredCopy {
    out:Arc<AtomicUsize>,
    installed:bool,
    pending:Option<(usize,f64)>,
    arm_this_pass:bool,
}

impl DeferredCopy {
    fn begin(&mut self,ctx:&egui::Context,selected:usize)->Option<f64>{
        if !self.installed {
            ctx.add_plugin(DeferredCopyOutput(self.out.clone()));
            self.installed=true;
        }
        self.arm_this_pass=false;
        let pending=self.pending.take();
        if self.out.swap(COPY_IDLE,Ordering::Relaxed)==COPY_EMITTED {
            return pending.filter(|(tab,_)|*tab==selected).map(|(_,until)|until);
        }
        None
    }

    fn finish(&mut self,ctx:&egui::Context,selected:usize){
        if !self.arm_this_pass {return;}
        // Capture the prefix only after all ordinary UI has rendered. Only a
        // copy appended by egui while ending this same pass can acknowledge.
        let before=ctx.output(|o|o.commands.len());
        self.out.store(before+COPY_ARMED_OFFSET,Ordering::Relaxed);
        self.pending=Some((selected,ctx.input(|i|i.time)+COPIED_SECONDS));
        ctx.request_repaint();
    }

    fn clear(&mut self){
        self.pending=None;
        self.arm_this_pass=false;
        self.out.store(COPY_IDLE,Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct TerminalUi {
    show: bool,
    capture: bool,
    copy_mode: bool,
    selection_stamp: Option<(usize,u64)>,
    drag_start: Option<egui::Pos2>,
    label_origin: bool,
    deferred_copy: DeferredCopy,
    paste_draft: Option<PasteDraft>,
    close_app: bool,
    tabs: Vec<TerminalTab>,
    next_tab_id: usize,
    selected: usize,
    notice: String,
}

#[derive(Default)]
struct TerminalTab {
    id: usize,
    name: String,
    target: String,
    destination: Option<(Device,Route)>,
    error: String,
    session: Option<Session>,
    demo: Option<vt100::Parser>,
    closing: bool,
    copied_until: Option<f64>,
}
impl TerminalTab {
    fn active(&self)->bool {self.session.as_ref().is_some_and(|s|s.state().is_active())}
    fn running(&self)->bool {self.session.as_ref().is_some_and(|s|matches!(s.state(),terminal::State::Running))}
    fn refers_to(&self,device:&Device)->bool{
        self.destination.as_ref().is_some_and(|(connected,route)|{
            connected.alias.eq_ignore_ascii_case(&device.alias)
                || (connected.user==device.user && connected.port==device.port &&
                    Route::ALL.iter().any(|candidate|!device.address(*candidate).is_empty() &&
                    device.address(*candidate).eq_ignore_ascii_case(connected.address(*route))))
        })
    }
    fn matches_target(&self,device:&Device,route:Route,demo:bool)->bool{
        self.demo.is_some()==demo && self.destination.as_ref().is_some_and(|(d,r)|
            d.user==device.user && d.port==device.port && d.address(*r).eq_ignore_ascii_case(device.address(route)))
    }
    fn stop(&mut self) {if let Some(s)=self.session.as_mut(){s.disconnect();}}
}
impl TerminalUi {
    fn active(&self)->bool {self.tabs.iter().any(TerminalTab::active)}
    pub(super) fn refers_to(&self,device:&Device)->bool{
        self.tabs.iter().any(|t|t.active() && t.refers_to(device))
    }
    pub(super) fn forget_closed(&mut self,device:&Device){
        for i in (0..self.tabs.len()).rev() {
            if !self.tabs[i].active() && self.tabs[i].refers_to(device){self.remove(i);}
        }
    }
    fn select(&mut self,i:usize){
        if self.selected!=i {
            if let Some(old)=self.tabs.get_mut(self.selected){old.copied_until=None;}
            self.deferred_copy.clear();
            self.paste_draft=None;
            self.label_origin=false;
            self.capture=false;self.copy_mode=false;self.selected=i;
        }
        self.notice.clear();
    }
    fn remove(&mut self,i:usize){
        if i==self.selected {
            self.deferred_copy.clear();
            self.paste_draft=None;
            self.label_origin=false;
            self.selection_stamp=None;
            self.copy_mode=false;
            self.capture=false;
        }
        self.tabs.remove(i);
        if i<self.selected {self.selected-=1;}
        self.selected=self.selected.min(self.tabs.len().saturating_sub(1));
    }
    fn close_tab(&mut self,i:usize){
        if i==self.selected {
            self.capture=false;
            self.paste_draft=None;
        }
        if self.tabs[i].active(){
            self.tabs[i].closing=true;
            self.tabs[i].stop();
        } else {self.remove(i);}
    }
    fn poll(&mut self){
        for t in &mut self.tabs {if let Some(s)=t.session.as_mut(){s.poll();}}
        for i in (0..self.tabs.len()).rev() {
            if self.tabs[i].closing && !self.tabs[i].active(){self.remove(i);}
        }
    }
    pub(super) fn finish_frame(&mut self,ctx:&egui::Context){
        if let Some(tab)=self.tabs.get(self.selected){self.deferred_copy.finish(ctx,tab.id);}
    }
    fn matching_tab(&self,device:&Device,route:Route,demo:bool)->Option<usize>{
        let matches=|t:&TerminalTab|t.matches_target(device,route,demo) && !t.closing
            && (demo || t.session.as_ref().is_some_and(|s|matches!(s.state(),terminal::State::Starting|terminal::State::Running)));
        if self.tabs.get(self.selected).is_some_and(matches){Some(self.selected)}
        else {self.tabs.iter().position(matches)}
    }
    pub(super) fn can_offer_new_tab(&self,device:&Device,route:Route)->bool{
        self.matching_tab(device,route,false).is_some()
    }
    pub(super) fn has_tab_space(&self)->bool{self.tabs.len()<MAX_TABS}
    pub(super) fn start(&mut self,device:&Device,route:Route,demo:bool){
        self.start_with(device,route,demo,false,|args|Session::start(args,12,82));
    }
    pub(super) fn start_new_tab(&mut self,device:&Device,route:Route){
        self.start_with(device,route,false,true,|args|Session::start(args,12,82));
    }
    fn unique_title(&self,base:&str,replacing:Option<usize>)->String{
        let in_use=|name:&str|self.tabs.iter().enumerate().any(|(i,t)|Some(i)!=replacing && t.name.eq_ignore_ascii_case(name));
        if !in_use(base){return base.to_owned();}
        (2..).map(|n|format!("{base} ({n})")).find(|name|!in_use(name)).unwrap()
    }
    fn start_with(&mut self,device:&Device,route:Route,demo:bool,new_tab:bool,spawn:impl FnOnce(&[String])->Result<Session,String>){
        self.show=true;
        let reuse=if new_tab {None}else{self.matching_tab(device,route,demo)};
        let existing=if new_tab || reuse.is_some(){None}else{self.tabs.iter().position(|t|
            t.matches_target(device,route,demo) && !t.active() && !t.closing)};
        if reuse.is_none() && existing.is_none() && !self.has_tab_space(){
            self.notice=format!("All {MAX_TABS} terminal tabs are in use. Close a tab before connecting.");
            return;
        }
        self.capture=false;
        self.copy_mode=false;
        self.notice.clear();
        self.selection_stamp=None;
        self.drag_start=None;
        self.paste_draft=None;
        self.deferred_copy.clear();
        self.label_origin=false;
        if let Some(i)=reuse{self.select(i);return;}
        let base=if device.name.is_empty(){&device.alias}else{&device.name};
        let title=self.unique_title(base,existing);
        self.next_tab_id+=1;
        let mut tab=TerminalTab {
            id:self.next_tab_id,
            name:title,
            target:format!("{}  |  {}",crate::endpoint(device,route),route.label()),
            destination:Some((device.clone(),route)),
            ..Default::default()
        };
        if demo {
            let mut vt=vt100::Parser::new(9,60,0);
            vt.process(b"DrakOps terminal preview: no SSH connection was made.\r\n\x1b[32mSample\x1b[0m> echo hello\r\nhello\r\n");
            tab.demo=Some(vt);
        } else {
            match device.ssh_args(route).and_then(|args|spawn(&args)){
                Ok(session)=>tab.session=Some(session),
                Err(error)=>tab.error=error,
            }
        }
        let i=if let Some(i)=existing {self.tabs[i]=tab;i} else {self.tabs.push(tab);self.tabs.len()-1};
        self.select(i);
    }
}

fn copy_gesture(ui:&egui::Ui,bounds:egui::Rect,enabled:bool,start:&mut Option<egui::Pos2>)->bool{
    if !enabled || !ui.input(|i|i.focused){*start=None;return false;}
    let (pressed,released,clicked,position)=ui.input(|i|{
        let p=&i.pointer;
        (p.button_pressed(egui::PointerButton::Primary),p.button_released(egui::PointerButton::Primary),p.primary_clicked(),p.interact_pos())
    });
    if pressed {
        *start=position.filter(|pos|bounds.contains(*pos) && ui.ctx().layer_id_at(*pos)==Some(ui.layer_id()));
    }
    if released {
        let origin=start.take();
        return !clicked && origin.zip(position).is_some_and(|(a,b)|bounds.contains(b) && a.distance(b)>6.);
    }
    false
}

fn scoped_copy(ui:&mut egui::Ui,copy:bool,render:impl FnOnce(&mut egui::Ui)){
    let injected=copy && ui.input_mut(|i|{
        if i.events.iter().any(|e|matches!(e,egui::Event::Copy)){false}
        else{i.events.push(egui::Event::Copy);true}
    });
    render(ui);
    if injected {ui.input_mut(|i|{if let Some(pos)=i.events.iter().rposition(|e|matches!(e,egui::Event::Copy)){i.events.remove(pos);}});}
}

fn vt_color(color: vt100::Color, default: Color32, muted: Color32) -> Color32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r,g,b) => Color32::from_rgb(r,g,b),
        vt100::Color::Idx(k) if k<16 => {
            const COLORS: [[u8;3];16]=[[10,10,13],[220,71,83],[60,172,116],[220,176,49],[85,138,235],[187,104,226],[26,168,189],[199,209,222],[113,125,146],[249,127,136],[124,224,157],[249,225,126],[132,173,253],[228,154,250],[112,220,232],[246,249,255]];
            let [r,g,b]=COLORS[k as usize];if k==0 { muted } else { Color32::from_rgb(r,g,b) }
        }
        vt100::Color::Idx(k) if k<232 => {
            let n=k-16;
            let level=|i:u8| if i==0 {0} else {55+40*i};
            Color32::from_rgb(level(n/36),level(n/6%6),level(n%6))
        }
        vt100::Color::Idx(k) => {let gray=8+10*(k-232);Color32::from_gray(gray)}
    }
}

fn input(event: &egui::Event, app_cursor: bool) -> Option<Vec<u8>> {
    match event {
        egui::Event::Text(s) if s.len() <= 4096 && !s.chars().any(char::is_control) => Some(s.as_bytes().to_vec()),
        egui::Event::Key{key, pressed:true, modifiers, ..} if !modifiers.mac_cmd && !modifiers.alt => {
            if modifiers.ctrl {
                let n=match key {Key::A=>1,Key::B=>2,Key::C=>3,Key::D=>4,Key::E=>5,Key::F=>6,Key::G=>7,Key::H=>8,Key::I=>9,Key::J=>10,Key::K=>11,Key::L=>12,Key::N=>14,Key::O=>15,Key::P=>16,Key::Q=>17,Key::R=>18,Key::S=>19,Key::T=>20,Key::U=>21,Key::W=>23,Key::X=>24,Key::Y=>25,Key::Z=>26,_=>return None};
                return Some(vec![n]);
            }
            let code = match key {
                Key::Enter => "\r", Key::Backspace => "\x7f", Key::Tab => "\t", Key::Escape => "\x1b",
                Key::ArrowUp => if app_cursor {"\x1bOA"} else {"\x1b[A"},
                Key::ArrowDown => if app_cursor {"\x1bOB"} else {"\x1b[B"},
                Key::ArrowRight => if app_cursor {"\x1bOC"} else {"\x1b[C"},
                Key::ArrowLeft => if app_cursor {"\x1bOD"} else {"\x1b[D"},
                Key::Home => "\x1b[H", Key::End => "\x1b[F",
                Key::Delete => "\x1b[3~", Key::PageUp => "\x1b[5~", Key::PageDown => "\x1b[6~",
                _ => return None,
            };
            Some(code.as_bytes().to_vec())
        }
        _ => None,
    }
}

fn render(ui: &mut egui::Ui, terminal: &vt100::Screen, focused: bool, row_height: f32) {
    let colors = theme::roles(ui.ctx());
    let font = FontId::new(13.,FontFamily::Monospace);
    let (rows,cols) = terminal.size();
    let cursor = terminal.cursor_position();
    ui.spacing_mut().item_spacing.y = 0.;
    for r in 0..rows {
        let mut job=LayoutJob::default();
        job.wrap.max_width=f32::INFINITY;
        for c in 0..cols {
            let Some(cell)=terminal.cell(r,c) else { continue };
            if cell.is_wide_continuation() { continue; }
            let mut fg=vt_color(cell.fgcolor(), colors.text, colors.muted);
            let mut bg=vt_color(cell.bgcolor(), Color32::TRANSPARENT, colors.background);
            if cell.inverse() { if bg==Color32::TRANSPARENT {bg=colors.background;} std::mem::swap(&mut fg,&mut bg); }
            if focused && !terminal.hide_cursor() && (r,c)==cursor { fg=colors.on_accent;bg=colors.accent; }
            let string=cell.contents();
            job.append(if string.is_empty() { " " } else { &string },0.,TextFormat{
                font_id:font.clone(), color:fg, background:bg, line_height:Some(row_height), italics:cell.italic(),
                underline:if cell.underline(){egui::Stroke::new(1.0_f32,fg)}else{egui::Stroke::NONE}, ..Default::default()
            });
        }
        ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend).selectable(true));
    }
}

const PASTE_LIMIT: usize = 4096;

fn paste_bytes(text:&str)->Result<Vec<u8>, &'static str>{
    let line=text.trim_end_matches(['\r','\n']);
    if line.is_empty(){return Err("The clipboard has no text to paste.");}
    if line.len()>PASTE_LIMIT {return Err("Paste is limited to 4 KiB.");}
    if line.chars().any(|c|c.is_control() || matches!(c,'\u{2028}'|'\u{2029}')) {
        return Err("Only single-line text without control characters can be pasted. Nothing was sent.");
    }
    Ok(line.as_bytes().to_vec())
}

#[derive(Default)]
struct TerminalInput {
    bytes:Vec<u8>,
    paste:Option<Result<Vec<u8>, &'static str>>,
    copy:bool,
}

fn input_batch(events:&mut Vec<egui::Event>,app_cursor:bool,mac:bool)->TerminalInput{
    let mut result=TerminalInput::default();
    let mut ambiguous=false;
    events.retain(|e|{
        match e {
            // Non-Mac native clipboard events erase the original chord.
            egui::Event::Paste(_) if !mac=>{ambiguous=true;return false;}
            egui::Event::Paste(s)=>{
                if result.paste.is_none(){result.paste=Some(paste_bytes(s));}
                return false;
            }
            egui::Event::Copy if !mac=>{ambiguous=true;return false;}
            egui::Event::Copy=>{result.copy=true;return false;}
            egui::Event::Cut=>return false, // No terminal cut.
            egui::Event::Key{key,pressed:true,modifiers,repeat,..}
                if (mac && modifiers.mac_cmd || !mac && *key==Key::C && modifiers.ctrl && modifiers.shift)
                   && !modifiers.alt && matches!(key,Key::C|Key::V) => {
                if !*repeat && *key==Key::C {result.copy=true;}
                return false;
            }
            _=>{}
        }
        let Some(part)=input(e,app_cursor) else{return true;};
        if result.bytes.len()+part.len()<=PASTE_LIMIT {result.bytes.extend(part);}
        false
    });
    // Native and synthesized shortcut can be delivered together. Prefer the
    // single native payload; never combine it with keystrokes (such as Enter).
    if ambiguous {return TerminalInput::default();}
    if result.paste.is_some() || result.copy{result.bytes.clear();}
    result
}

#[derive(Default)]
struct PasteDraft {
    text:String,
    first_frame:bool,
}

enum PasteDecision {Keep,Cancel,Send(Vec<u8>)}

impl PasteDraft {
    fn new()->Self {Self{first_frame:true,..Default::default()}}

    fn show(&mut self,ctx:&egui::Context,target:&str)->PasteDecision {
        let modal=egui::Modal::new(egui::Id::new("terminal-local-paste")).show(ctx,|ui|{
            ui.heading("Paste into terminal");
            ui.monospace(target);
            ui.label("Paste into this local field, then choose Send to terminal. It won’t press Enter.");
            ui.weak("After sending, click the terminal to resume typing.");
            let mut input=egui::TextEdit::multiline(&mut self.text)
                .id(egui::Id::new("terminal-local-paste-private-input")).desired_rows(2).desired_width(410.)
                .hint_text(if cfg!(target_os="macos") {"Cmd+V to paste"} else {"Ctrl+V to paste"})
                .show(ui);
            // egui stores undo history behind a shared state handle. Clear it
            // every render so cancelled/sent drafts never survive in that state.
            input.state.clear_undoer();
            if self.first_frame {input.response.request_focus();self.first_frame=false;}
            let payload=paste_bytes(&self.text);
            if !self.text.is_empty() && let Err(e)=&payload{ui.weak(*e);}
            ui.horizontal(|ui|{
                if ui.button("Cancel").clicked(){return PasteDecision::Cancel;}
                if ui.add_enabled(payload.is_ok(),egui::Button::new("Send to terminal")).clicked()
                    && let Ok(bytes)=payload {return PasteDecision::Send(bytes);}
                PasteDecision::Keep
            }).inner
        });
        if modal.should_close(){PasteDecision::Cancel}else{modal.inner}
    }
}

fn keyboard(ui: &mut egui::Ui, hit: &egui::Response, requested: bool, running: bool, captured: &mut bool, app_cursor: bool) -> TerminalInput {
    let (focused, body_click, clicked_outside, dragging, release) = ui.input(|i| {
        let p = &i.pointer;
        let at = p.interact_pos();
        let inside = at.filter(|pos| p.primary_clicked() && hit.rect.contains(*pos));
        (i.focused && !i.events.iter().any(|e|matches!(e,egui::Event::WindowFocused(false))), inside, p.any_pressed() && at.is_some_and(|pos| !hit.rect.contains(pos)),
          p.is_decidedly_dragging() && p.press_origin().is_some_and(|pos| hit.rect.contains(pos)),
          i.key_pressed(Key::Escape) && i.modifiers.ctrl)
    });
    if !focused || !running || release || dragging {
        *captured = false;
        ui.memory_mut(|m|m.surrender_focus(hit.id));
        return TerminalInput::default();
    }
    let clicked_body = body_click.is_some_and(|pos| ui.ctx().layer_id_at(pos) == Some(ui.layer_id()));
    let keyboard_activate=!(*captured) && hit.has_focus() && ui.input(|i|i.events.iter().any(|e|
        matches!(e,egui::Event::Key{key:Key::F2,pressed:true,repeat:false,modifiers,..}
            if *modifiers==egui::Modifiers::NONE)));
    if requested || clicked_body || keyboard_activate {
        // Focus is explicitly armed; moving widget focus alone never enables SSH typing.
        hit.request_focus();
        *captured = true;
        // Do not pass the key/mouse event used to acquire capture through to SSH.
        return TerminalInput::default();
    }
    if clicked_outside || !hit.has_focus() {
        *captured = false;
        return TerminalInput::default();
    }
    if !*captured { return TerminalInput::default(); }
    ui.memory_mut(|m|m.set_focus_lock_filter(hit.id, egui::EventFilter { tab:true, horizontal_arrows:true, vertical_arrows:true, escape:true }));
    ui.input_mut(|i| input_batch(&mut i.events,app_cursor,cfg!(target_os="macos")))
}

impl App {
    pub(super) fn terminal_panel(&mut self, ctx: &egui::Context, _frame:&eframe::Frame) {
        let auto_copy=self.book.copy_on_select;
        let deck=&mut self.terminal;
        deck.poll();
        let selected_id=deck.tabs.get(deck.selected).map_or(0,|t|t.id);
        if let Some(until)=deck.deferred_copy.begin(ctx,selected_id)
            && let Some(tab)=deck.tabs.get_mut(deck.selected)
        {tab.copied_until=Some(until);}
        if deck.active() && ctx.input(|i|i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            deck.close_app=true;
            deck.capture=false;
            deck.paste_draft=None;
            for tab in &mut deck.tabs {tab.stop();}
        }
        if deck.close_app && !deck.active(){
            deck.close_app=false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if deck.active(){ctx.request_repaint_after(Duration::from_millis(30));}
        if deck.tabs.is_empty(){return;}
        if !deck.show {
            deck.capture=false;
            deck.paste_draft=None;
            deck.drag_start=None;
            egui::TopBottomPanel::bottom("ssh-collapsed").show(ctx,|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("SSH terminal · {} tab(s)",deck.tabs.len()));
                    if ui.button("Show terminal").clicked(){deck.show=true;}
                });
            });
            return;
        }
        let mut close_requested=None;
        let max_h=(ctx.content_rect().height()*0.53).max(125.);
        egui::TopBottomPanel::bottom("homeops-terminal")
            .resizable(true).min_height(125.).max_height(max_h).default_height(236.)
            .show(ctx, |ui| {
                let mut choose=None;
                let mut background_close_pointer=false;
                egui::ScrollArea::horizontal().id_salt("ssh-tabs").max_height(30.).show(ui,|ui|{
                    ui.horizontal(|ui|for (i,tab) in deck.tabs.iter().enumerate(){
                        let state=tab.session.as_ref().map(|s|s.state().label()).unwrap_or_else(||if tab.demo.is_some(){"Preview".into()}else{"Failed".into()});
                        ui.push_id(tab.id,|ui|ui.horizontal(|ui|{
                        ui.spacing_mut().item_spacing.x=3.;
                        if ui.selectable_label(i==deck.selected,format!("{} · {}",tab.name,state)).on_hover_text(&tab.target).clicked(){choose=Some(i);}
                        let label=format!("Close SSH tab: {}",tab.name);
                        let response=ui.add_enabled(!tab.closing,egui::Button::new("×").small().min_size(egui::vec2(20.,20.)));
                        response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::Button,!tab.closing,&label));
                        let response=response.on_hover_text(label);
                        if i!=deck.selected && (response.is_pointer_button_down_on() || response.clicked()){
                            background_close_pointer=true;
                        }
                        if response.clicked(){close_requested=Some(i);}
                        }));
                    });
                });
                let switched=choose.is_some_and(|i|i!=deck.selected);
                if let Some(i)=choose{deck.select(i);ctx.plugin::<egui::text_selection::LabelSelectionState>().lock().clear_selection();}
                if !deck.notice.is_empty(){ui.colored_label(theme::roles(ctx).error,&deck.notice);}
                let t=&mut deck.tabs[deck.selected];
                let tab_id=t.id;
                use std::hash::{Hash,Hasher};
                let mut hash=std::collections::hash_map::DefaultHasher::new();
                if let Some(s)=&t.session {s.screen().contents().hash(&mut hash);}
                if let Some(p)=&t.demo {p.screen().contents().hash(&mut hash);}
                let stamp=(tab_id, hash.finish());
                if deck.selection_stamp!=Some(stamp){
                    deck.drag_start=None;
                    deck.label_origin=false;
                    ctx.plugin::<egui::text_selection::LabelSelectionState>().lock().clear_selection();
                    deck.selection_stamp=Some(stamp);
                }
                let focus=ui.make_persistent_id(("terminal-input-focus",tab_id));
                let mut paste_click=false;
                let mut copy_click=false;
                ui.horizontal_wrapped(|ui| {
                    ui.strong("SSH terminal");
                    ui.label(RichText::new(&t.target).monospace());
                    if let Some(s)=&t.session{ui.weak(s.state().label());}
                    if t.demo.is_some(){ui.weak("Read-only sample. No SSH connection.");}
                    if t.session.as_ref().is_some_and(|s| matches!(s.state(),terminal::State::Running)) {
                        paste_click=ui.small_button("Paste").on_hover_text("Review one line locally before sending").clicked();
                        if ui.small_button(if deck.copy_mode {"Back to terminal"}else{"Select text"}).clicked(){deck.copy_mode=!deck.copy_mode;deck.capture=false;deck.drag_start=None;}
                        if deck.copy_mode {copy_click=ui.small_button("Copy selection").clicked();}
                        ui.weak(if deck.capture {"Keyboard active | Ctrl+Esc to release"} else {"Click terminal to type | Drag to select | Tab then F2 for keyboard"});
                    }
                    if let Some(s)=&t.session && matches!(s.state(),terminal::State::Running) && ui.button("Send Ctrl+C").clicked(){
                        if let Err(e)=s.send(&[3]){t.error=e;}
                    }
                    if t.active() && ui.button("Disconnect").clicked(){deck.capture=false;t.stop();}
                    if ui.add_enabled(!t.closing,egui::Button::new("Close tab")).clicked(){close_requested=Some(deck.selected);}
                    if ui.button("Collapse").clicked(){deck.show=false;deck.capture=false;}
                    if t.session.is_some(){copied_badge(ui,t.copied_until);}
                });
                let close_tab=close_requested==Some(deck.selected);
                if !t.error.is_empty(){ui.colored_label(theme::roles(ctx).error,&t.error);}
                let failed=t.session.as_ref().is_some_and(|s|matches!(s.state(),terminal::State::Failed(_) | terminal::State::Exited(1..)))
                    || (t.session.is_none() && t.destination.is_some() && !t.error.is_empty());
                if failed {
                    ui.collapsing("Connection help",|ui| {
                        ui.label("New host? Verify its fingerprint independently before accepting it. For a changed key, stop and verify why it changed; do not accept it blindly.");
                        ui.label("Permission denied or publickey? Check the saved username and your local SSH agent/key. Timed out or refused? Check the route, port, and whether SSH is running on the target.");
                    });
                    if let Some((device,route))=&t.destination {
                        ui.horizontal_wrapped(|ui|{
                            if ui.button("Open in external terminal").clicked(){
                                match crate::external_terminal::open(device,*route){
                                    Ok(())=>t.error="External terminal launch requested. Follow prompts there.".into(),
                                    Err(e)=>t.error=e,
                                }
                            }
                            if ui.button("Copy command").clicked() && let Ok(command)=device.ssh(*route){ctx.copy_text(command);}
                        });
                    }
                }
                if t.session.is_none() && t.demo.is_none() { return; }
                let h=ui.available_height().max(28.);
                let width=ui.available_width();
                let dims=(h / 16.).floor().clamp(2., f32::from(terminal::MAX_ROWS)) as u16;
                let cw=ui.fonts_mut(|f|f.glyph_width(&FontId::new(13.,FontFamily::Monospace),'M')).max(1.);
                let cols=((width - 16.) / cw).floor().clamp(2., f32::from(terminal::MAX_COLS)) as u16;
                let rect=ui.available_rect_before_wrap();
                let hit=ui.interact(rect,focus,egui::Sense::focusable_noninteractive());
                hit.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::Other,true,"SSH terminal. Press F2 to enable typing."));
                let focus_outline=hit.has_focus();
                let running=deck.show && !switched && !close_tab && !deck.close_app && t.running();
                let app_cursor=t.session.as_ref().is_some_and(|s| s.screen().application_cursor());
                let focused=ctx.input(|i|i.focused && !i.events.iter().any(|e|matches!(e,egui::Event::WindowFocused(false))));
                let paste_was_open=deck.paste_draft.is_some();
                if paste_click {deck.copy_mode=false;}
                if paste_click && running && focused {
                    deck.paste_draft=Some(PasteDraft::new());
                }
                let mut submitted=None;
                if let Some(draft)=&mut deck.paste_draft {
                    if !running || !focused {deck.paste_draft=None;}
                    else {
                        match draft.show(ctx,&t.target) {
                            PasteDecision::Keep=>{}
                            PasteDecision::Cancel=>deck.paste_draft=None,
                            PasteDecision::Send(bytes)=>{submitted=Some(bytes);deck.paste_draft=None;}
                        }
                    }
                }
                let blocked=paste_was_open || paste_click;
                // The background tab's close control is not an instruction to
                // activate it or release the current session's keyboard capture.
                let preserve_capture=background_close_pointer && deck.capture;
                let routed=keyboard(ui,&hit,preserve_capture,running && !deck.copy_mode && !blocked,&mut deck.capture,app_cursor);
                if routed.copy {deck.copy_mode=true;deck.capture=false;}
                let bytes=if let Some(bytes)=submitted {
                    // Leave terminal capture released. Even a held Enter or
                    // queued key from the editor cannot reach SSH after closing.
                    deck.capture=false;t.error.clear();bytes
                } else if running && deck.capture && !deck.copy_mode && !blocked {
                    match routed.paste {
                        Some(Ok(bytes))=>{t.error.clear();bytes}
                        Some(Err(e))=>{t.error=e.into();Vec::new()}
                        None=>routed.bytes,
                    }
                }else{Vec::new()};
                let has_focus=deck.capture;
                if let Some(s)=t.session.as_mut(){
                    if s.state().is_active() && s.screen().size()!=(dims,cols) {
                        let _=s.resize(dims,cols);
                    }
                    if !bytes.is_empty() && let Err(e)=s.send(&bytes){t.error=e;}
                }
                let colors=theme::roles(ctx);
                egui::Frame::new().fill(colors.background).stroke(egui::Stroke::new(1.0_f32,if has_focus || focus_outline{colors.accent}else{colors.border})).inner_margin(5).show(ui,|ui| {
                    ui.set_min_width((width-12.).max(25.));
                    egui::ScrollArea::both().id_salt(("terminal-body",tab_id)).auto_shrink([false,false]).max_height((h-11.).max(12.)).show(ui,|ui| {
                        if let Some(s)=&t.session{
                            if deck.copy_mode {
                                use std::hash::{Hash,Hasher};
                                let contents=s.screen().contents();
                                let mut stamp=std::hash::DefaultHasher::new();
                                contents.hash(&mut stamp);
                                let mut text=contents.as_str();
                                ui.weak("Select visible text to copy. No keystrokes go to SSH in this view.");
                                observe_copy(ctx,&mut t.copied_until, ||{
                                    let output=egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace)
                                        .id_salt(("terminal-copy",tab_id,stamp.finish())).show(ui);
                                    let range=output.cursor_range.or_else(||egui::TextEdit::load_state(ctx,output.response.id).and_then(|s|s.cursor.char_range()));
                                    if copy_click && let Some(range)=range.filter(|r|!r.is_empty()){
                                        let range=range.as_sorted_char_range();
                                        ctx.copy_text(contents.chars().skip(range.start).take(range.len()).collect::<String>());
                                    }
                                });
                            }else{
                                let bounds=ui.clip_rect().intersect(rect);
                                let copy=copy_gesture(ui,bounds,auto_copy && !blocked && deck.show && !switched && !close_tab && !deck.close_app,&mut deck.drag_start);
                                if ui.input(|i|i.pointer.primary_pressed()) {
                                    let pointer=ui.input(|i|i.pointer.interact_pos());
                                    deck.label_origin=pointer.is_some_and(|pos|bounds.contains(pos)
                                        && ctx.layer_id_at(pos)==Some(ui.layer_id()));
                                }
                                let manual=deck.label_origin
                                    && ui.input(|i|i.events.iter().any(|e|matches!(e,egui::Event::Copy)))
                                    && ctx.memory(|m|m.focused().is_none_or(|id|id==focus));
                                let selected=if manual {
                                    let labels=ctx.plugin::<egui::text_selection::LabelSelectionState>();
                                    let has_selection=labels.lock().has_selection();
                                    has_selection
                                } else {false};
                                if (copy || selected) && !blocked && deck.show && !switched && !close_tab && !deck.close_app {
                                    deck.deferred_copy.arm_this_pass=true;
                                }
                                scoped_copy(ui,copy,|ui|render(ui,s.screen(),has_focus,16.));
                            }
                        }
                        if let Some(parser)=&t.demo{render(ui,parser.screen(),false,16.);}
                    });
                });
            });
        if let Some(i)=close_requested {deck.close_tab(i);}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;
    #[test]
    fn terminal_text_does_not_execute_clipboard_or_rewrite_controls() {
        assert_eq!(input(&egui::Event::Paste("echo x\n".into()), false), None);
        assert_eq!(input(&egui::Event::Text("\x1b]52;x".into()), false), None);
        assert_eq!(input(&egui::Event::Text("hello".into()), false), Some(b"hello".to_vec()));
        fn key(key: Key, modifiers:Modifiers) -> egui::Event {
            egui::Event::Key{key,physical_key:None,pressed:true,repeat:false,modifiers}
        }
        assert_eq!(input(&key(Key::C,Modifiers::CTRL),false),Some(vec![3]));
        assert_eq!(input(&key(Key::ArrowUp,Modifiers::NONE),true),Some(b"\x1bOA".to_vec()));
        assert_eq!(input(&key(Key::Enter,Modifiers::NONE),false),Some(vec![13]));
    }
    #[test]
    fn gui_body_click_or_explicit_f2_arms_keyboard_and_outside_stops_capture() {
        let ctx=egui::Context::default();
        let mut captured=false;
        let mut body_center=egui::Pos2::ZERO;
        let mut received=Vec::new();
        let mut frame=|events:Vec<egui::Event>| {
            let raw=egui::RawInput { screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(800.,600.))), focused:true, events, ..Default::default() };
            let _=ctx.run(raw,|ctx| {
                egui::CentralPanel::default().show(ctx,|ui| {
                    let id=ui.make_persistent_id("terminal-input-focus");
                    let rect=ui.allocate_space(egui::vec2(530.,230.)).1;
                    body_center=rect.center();
                    let body=ui.interact(rect,id,egui::Sense::focusable_noninteractive());
                    received.extend(keyboard(ui,&body,false,true,&mut captured,false).bytes);
                });
            });
            (body_center, captured, std::mem::take(&mut received))
        };
        let button=|pos,pressed|egui::Event::PointerButton { pos, button:egui::PointerButton::Primary, pressed, modifiers:Modifiers::NONE };
        let target=frame(vec![]).0;
        frame(vec![egui::Event::PointerMoved(target),button(target,true)]);
        let (_, enabled, initial_bytes)=frame(vec![button(target,false),egui::Event::Text("ignore acquisition".into())]);
        assert!(enabled);
        assert!(initial_bytes.is_empty());
        let enter=egui::Event::Key { key:Key::Enter, physical_key:None, pressed:true, repeat:false, modifiers:Modifiers::NONE };
        let sent=frame(vec![egui::Event::Text("fixture".into()),enter]).2;
        assert_eq!(sent,b"fixture\r");
        let mut local=crate::terminal::tests::fixture();
        crate::terminal::tests::until(&mut local,|s| matches!(s.state(),terminal::State::Running) && s.screen().contents().contains("[fixture-ready]"));
        local.send(&sent).unwrap();
        crate::terminal::tests::until(&mut local,|s| matches!(s.state(),terminal::State::Exited(0)) && s.screen().contents().contains("[fixture-got:fixture]"));
        let outside=egui::pos2(760.,540.);
        assert!(!frame(vec![egui::Event::PointerMoved(outside),button(outside,true),egui::Event::Text("outside".into())]).1);
        assert!(frame(vec![button(outside,false),egui::Event::Text("never sent".into())]).2.is_empty());
        // Assign widget focus as Tab would. It must not enable SSH by itself.
        let _=ctx.run(egui::RawInput::default(),|ctx|egui::CentralPanel::default().show(ctx,|ui|
            ctx.memory_mut(|m|m.request_focus(ui.make_persistent_id("terminal-input-focus")))).inner);
        let focused=frame(vec![egui::Event::Text("do not forward".into())]);
        assert!(!focused.1 && focused.2.is_empty());
        let f2=egui::Event::Key{key:Key::F2,physical_key:None,pressed:true,repeat:false,modifiers:Modifiers::NONE};
        let enabled=frame(vec![f2,egui::Event::Text("discard activation batch".into())]);
        assert!(enabled.1 && enabled.2.is_empty());
        let regained=frame(vec![egui::Event::WindowFocused(false),egui::Event::WindowFocused(true),egui::Event::Text("discard focus churn".into())]);
        assert!(!regained.1 && regained.2.is_empty());
    }

    #[test]
    fn copy_on_select_only_copies_one_completed_drag_when_enabled() {
        for enabled in [false,true] {
            let ctx=egui::Context::default();
            let mut vt=vt100::Parser::new(3,40,0);
            vt.process(b"ABCDEFGHIJKL\r\nsecond line");
            let mut tracked=None;
            let mut relay=DeferredCopy::default();
            let mut origin=egui::Pos2::ZERO;
            let mut frame=|events, reset| {
                if reset {tracked=None;ctx.plugin::<egui::text_selection::LabelSelectionState>().lock().clear_selection();}
                let raw=egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(420.,260.))),focused:true,events,..Default::default()};
                let output=ctx.run(raw,|ctx|{
                    relay.begin(ctx,0);
                    egui::CentralPanel::default().show(ctx,|ui|{
                        let bounds=ui.available_rect_before_wrap();
                        origin=bounds.min;
                        let copy=copy_gesture(ui,bounds,enabled,&mut tracked);
                        relay.arm_this_pass=copy;
                        scoped_copy(ui,copy,|ui|render(ui,vt.screen(),false,16.));
                    });
                    relay.finish(ctx,0);
                });
                let acknowledged=relay.out.load(Ordering::Relaxed)==COPY_EMITTED;
                assert_eq!(acknowledged,output.platform_output.commands.iter().any(|c|
                    matches!(c,egui::OutputCommand::CopyText(value) if !value.is_empty())));

                (origin,output.platform_output.commands.into_iter().filter_map(|c|match c{egui::OutputCommand::CopyText(s)=>Some(s),_=>None}).collect::<Vec<_>>())
            };
            let origin=frame(vec![],false).0;
            let start=origin+egui::vec2(2.,7.);
            let end=origin+egui::vec2(48.,7.);
            let click=|pos,pressed|egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE};
            assert!(frame(vec![egui::Event::PointerMoved(start),click(start,true)],false).1.is_empty());
            assert!(frame(vec![egui::Event::PointerMoved(end)],false).1.is_empty());
            let copied=frame(vec![click(end,false)],false).1;
            if enabled {assert_eq!(copied.len(),1); assert_eq!(copied[0],"ABCDEF");}
            else{assert!(copied.is_empty());}
            assert!(frame(vec![],false).1.is_empty());
            frame(vec![egui::Event::PointerMoved(start),click(start,true)],false);
            assert!(frame(vec![click(start,false)],false).1.is_empty());
            frame(vec![click(start,true)],false);
            frame(vec![egui::Event::PointerMoved(end)],false);
            assert!(frame(vec![click(end,false)],true).1.is_empty());
        }
    }

    #[test]
    fn deferred_copy_ignores_ordinary_copy_in_multipass_and_discards_stale_tab(){
        let ctx=egui::Context::default();
        let mut relay=DeferredCopy::default();
        let _=ctx.run(egui::RawInput::default(),|ctx|{
            assert!(relay.begin(ctx,0).is_none());
            // Other widgets may emit before finish, including in a discarded pass.
            ctx.copy_text("ordinary fixture".into());
            relay.arm_this_pass=true;
            relay.finish(ctx,0);
            if ctx.current_pass_index()==0 {ctx.request_discard("exercise second pass");}
        });
        assert_ne!(relay.out.load(Ordering::Relaxed),COPY_EMITTED);
        relay.pending=Some((0,8.));
        relay.out.store(COPY_EMITTED,Ordering::Relaxed);
        assert_eq!(relay.begin(&ctx,1),None);
        assert_eq!(relay.begin(&ctx,0),None);
    }

    #[test]
    fn explicit_copy_badge_stays_tab_local_ignores_empty_and_expires(){
        let ctx=egui::Context::default();
        let mut tabs=[TerminalTab::default(),TerminalTab::default()];
        let frame=|time:f64,tab:usize,value:Option<&str>,tabs:&mut [TerminalTab;2]|{
            let _=ctx.run(egui::RawInput{time:Some(time),focused:true,..Default::default()},|ctx|{
                // A separate control's earlier clipboard command must not arm a badge.
                ctx.copy_text("outside".into());
                egui::CentralPanel::default().show(ctx,|ui|{
                    copied_badge(ui,tabs[tab].copied_until);
                    observe_copy(ctx,&mut tabs[tab].copied_until,||{
                        if let Some(value)=value{ctx.copy_text(value.into());}
                    });
                });
            });
        };
        frame(1.,0,Some("first"),&mut tabs);
        frame(1.1,1,None,&mut tabs);
        assert_eq!(tabs[1].copied_until,None);
        frame(1.2,0,Some("second"),&mut tabs);
        assert_eq!(tabs[0].copied_until,Some(3.2));
        frame(1.3,1,Some(""),&mut tabs);
        assert_eq!(tabs[1].copied_until,None);
        frame(3.3,0,None,&mut tabs);
        assert!(tabs[0].copied_until.unwrap()<3.3);
        frame(3.4,1,Some("third"),&mut tabs);
        assert_eq!(tabs[0].copied_until,Some(3.2));
        assert_eq!(tabs[1].copied_until,Some(5.4));
    }

    #[test]
    fn drag_selects_visible_output_and_copies_without_keyboard_capture() {
        let ctx=egui::Context::default();
        let mut vt=vt100::Parser::new(3,40,0);
        vt.process(b"ABCDEFGHIJKL\r\nsecond line");
        let mut origin=egui::Pos2::ZERO;
        let mut frame=|events| {
            let raw=egui::RawInput{screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(420.,260.))),focused:true,events,..Default::default()};
            let output=ctx.run(raw,|ctx| {
                egui::CentralPanel::default().show(ctx,|ui|{
                    origin=ui.cursor().min;
                    render(ui,vt.screen(),false,16.);
                });
            });
            (origin,output.platform_output.commands)
        };
        let origin=frame(vec![]).0;
        let start=origin+egui::vec2(2.,7.);
        let end=origin+egui::vec2(48.,7.);
        let click=|pos,pressed|egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE};
        frame(vec![egui::Event::PointerMoved(start),click(start,true)]);
        frame(vec![egui::Event::PointerMoved(end)]);
        frame(vec![click(end,false)]);
        let (_,commands)=frame(vec![egui::Event::Copy]);
        let text=commands.into_iter().find_map(|c|match c{egui::OutputCommand::CopyText(s)=>Some(s),_=>None}).expect("visible drag selection copied");
        assert!(!text.is_empty() && "ABCDEFGHIJKL".contains(&text) && text.len()<12, "{text:?}");
        ctx.plugin::<egui::text_selection::LabelSelectionState>().lock().clear_selection();
        assert!(!frame(vec![egui::Event::Copy]).1.iter().any(|c|matches!(c,egui::OutputCommand::CopyText(_))));
    }

    #[test]
    fn staged_paste_uses_local_editor_and_enter_never_submits(){
        let ctx=egui::Context::default();
        let mut draft=PasteDraft::new();
        let raw=|events|egui::RawInput{
            events,focused:true,screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(700.,500.))),
            ..Default::default()
        };
        let _=ctx.run(raw(vec![]),|ctx|assert!(matches!(draft.show(ctx,"fixture@example:22"),PasteDecision::Keep)));
        let enter=egui::Event::Key{key:Key::Enter,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers::NONE};
        let _=ctx.run(raw(vec![egui::Event::Paste("safe marker".into()),enter]),|ctx|
            assert!(matches!(draft.show(ctx,"fixture@example:22"),PasteDecision::Keep)));
        assert_eq!(paste_bytes(&draft.text),Ok(b"safe marker".to_vec()));
        let mut deck=TerminalUi{paste_draft:Some(draft),tabs:vec![TerminalTab::default(),TerminalTab::default()],..Default::default()};
        deck.select(1);
        assert!(deck.paste_draft.is_none());
        deck.paste_draft=Some(PasteDraft::new());
        deck.remove(1);
        assert!(deck.paste_draft.is_none());
    }

    #[test]
    fn reopened_paste_editor_cannot_undo_into_a_cancelled_draft(){
        let ctx=egui::Context::default();
        let raw=|events|egui::RawInput{
            events,focused:true,screen_rect:Some(egui::Rect::from_min_size(egui::Pos2::ZERO,egui::vec2(700.,500.))),
            ..Default::default()
        };
        let mut old=PasteDraft::new();
        let _=ctx.run(raw(vec![]),|ctx|{old.show(ctx,"fixture-one");});
        let _=ctx.run(raw(vec![egui::Event::Paste("temporary fixture only".into())]),|ctx|{old.show(ctx,"fixture-one");});
        assert!(!old.text.is_empty());
        drop(old);
        let mut new=PasteDraft::new();
        let _=ctx.run(raw(vec![]),|ctx|{new.show(ctx,"fixture-two");});
        let undo=egui::Event::Key{key:Key::Z,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers::COMMAND};
        let _=ctx.run(raw(vec![undo]),|ctx|assert!(matches!(new.show(ctx,"fixture-two"),PasteDecision::Keep)));
        assert!(new.text.is_empty());
    }

    #[test]
    fn clipboard_batch_requires_explicit_intent_and_never_pairs_with_enter(){
        let key=|key,modifiers|egui::Event::Key{key,physical_key:None,pressed:true,repeat:false,modifiers};
        assert_eq!(paste_bytes("echo hi\r\n").unwrap(),b"echo hi");
        for invalid in ["","a\nb","a\tb","a\x1b[31m","a\u{2028}b"] {assert!(paste_bytes(invalid).is_err());}
        assert!(paste_bytes(&"x".repeat(PASTE_LIMIT+1)).is_err());
        for event in [egui::Event::Copy,egui::Event::Paste("fixture".into())] {
            let mut events=vec![event,key(Key::Enter,egui::Modifiers::NONE)];
            let result=input_batch(&mut events,false,false);
            assert!(result.bytes.is_empty() && !result.copy && result.paste.is_none());
        }
        let mut events=vec![key(Key::V,egui::Modifiers::MAC_CMD),egui::Event::Paste("one\r\n".into()),key(Key::Enter,egui::Modifiers::NONE)];
        let result=input_batch(&mut events,false,true);
        assert!(result.bytes.is_empty());
        assert_eq!(result.paste.unwrap().unwrap(),b"one");
        let mut events=vec![egui::Event::Copy];
        assert!(input_batch(&mut events,false,true).copy);
    }

    #[test]
    fn independent_tabs_keep_running_and_only_selected_receives_input() {
        use crate::terminal::tests::{fixture,until};
        fn device(alias:&str,host:&str)->Device {
            Device{alias:alias.into(),host:host.into(),user:"test".into(),..Default::default()}
        }
        let dev=device("dev","192.0.2.2");
        let prod=device("prod","192.0.2.3");
        let mut deck=TerminalUi::default();
        for d in [&dev,&prod] {
            let mut s=fixture();
            until(&mut s,|s|matches!(s.state(),terminal::State::Running) && s.screen().contents().contains("[fixture-ready]"));
            deck.tabs.push(TerminalTab {destination:Some((d.clone(),Route::Saved)),session:Some(s),..Default::default()});
        }
        assert!(deck.refers_to(&dev) && deck.refers_to(&prod));
        deck.capture=true; deck.select(1);
        assert!(!deck.capture);
        deck.start(&prod,Route::Saved,false);
        assert_eq!(deck.tabs.len(),2);
        let prod_session=deck.tabs[deck.selected].session.as_mut().unwrap();
        prod_session.send(&paste_bytes("wait\r\n").unwrap()).unwrap();
        std::thread::sleep(Duration::from_millis(75));
        prod_session.poll();
        assert!(!prod_session.screen().contents().contains("[fixture-got:"));
        prod_session.send(b"\r").unwrap();
        until(prod_session,|s|s.screen().contents().contains("[fixture-got:wait]"));
        let dev_session=deck.tabs[0].session.as_ref().unwrap();
        assert!(!dev_session.screen().contents().contains("[fixture-got:"));
        assert!(deck.tabs[0].running());
        deck.close_tab(deck.selected);
        for _ in 0..150 {
            deck.poll();
            if deck.tabs.len()==1 {break;}
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(deck.tabs.len(),1);
        assert_eq!(deck.selected,0);
        assert!(deck.tabs[0].running());
        assert!(deck.refers_to(&dev) && !deck.refers_to(&prod));
        let dev_session=deck.tabs[deck.selected].session.as_mut().unwrap();
        dev_session.send(b"only-dev\r").unwrap();
        until(dev_session,|s|matches!(s.state(),terminal::State::Exited(0)) && s.screen().contents().contains("[fixture-got:only-dev]"));
        assert!(!deck.active());
    }

    #[test]
    fn closing_background_session_keeps_selected_session_and_capture(){
        use crate::terminal::tests::{fixture,until};
        let mut deck=TerminalUi::default();
        for id in [11,22] {
            let mut session=fixture();
            until(&mut session,|s|matches!(s.state(),terminal::State::Running) && s.screen().contents().contains("[fixture-ready]"));
            deck.tabs.push(TerminalTab{id,session:Some(session),..Default::default()});
        }
        deck.select(1);
        deck.capture=true;
        deck.selection_stamp=Some((22,9));
        deck.close_tab(0);
        assert_eq!(deck.tabs[deck.selected].id,22);
        assert!(deck.capture && deck.tabs[0].closing);
        for _ in 0..150 {
            deck.poll();
            if deck.tabs.len()==1 {break;}
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(deck.tabs.len(),1);
        assert_eq!(deck.tabs[deck.selected].id,22);
        assert!(deck.capture && deck.tabs[deck.selected].running());
        assert_eq!(deck.selection_stamp,Some((22,9)));
        let active=deck.tabs[deck.selected].session.as_mut().unwrap();
        active.send(b"still-active\r").unwrap();
        until(active,|s|s.screen().contents().contains("[fixture-got:still-active]"));
    }

    #[test]
    fn explicit_same_endpoint_tab_is_independent_and_default_connect_reuses_it(){
        use crate::terminal::tests::{fixture,until};
        let d=Device{alias:"dev".into(),name:"DLMS DEV".into(),host:"192.0.2.13".into(),user:"tester".into(),..Default::default()};
        let mut deck=TerminalUi::default();
        assert!(!deck.can_offer_new_tab(&d,Route::Saved));
        let fake=|args:&[String]|{
            assert_eq!(args,d.ssh_args(Route::Saved).unwrap());
            Ok(fixture())
        };
        deck.start_with(&d,Route::Saved,false,false,fake);
        let original=deck.tabs[0].id;
        assert!(deck.can_offer_new_tab(&d,Route::Saved));
        deck.start_with(&d,Route::Saved,false,false,|_|panic!("default Connect must not spawn"));
        assert_eq!(deck.tabs.len(),1);
        deck.capture=true;
        deck.paste_draft=Some(PasteDraft::new());
        deck.start_with(&d,Route::Saved,false,true,fake);
        assert_eq!(deck.tabs.len(),2);
        assert_eq!(deck.tabs[0].name,"DLMS DEV");
        assert_eq!(deck.tabs[1].name,"DLMS DEV (2)");
        assert_ne!(deck.tabs[1].id,original);
        assert_eq!(deck.selected,1);
        assert!(!deck.capture && deck.paste_draft.is_none());
        deck.start_with(&d,Route::Saved,false,false,|_|panic!("already-selected healthy tab must be reused"));
        assert_eq!(deck.selected,1);
        for t in &mut deck.tabs {
            until(t.session.as_mut().unwrap(),|s|s.screen().contents().contains("[fixture-ready]"));
        }
        let additional=deck.tabs[1].session.as_mut().unwrap();
        additional.send(b"second-only\r").unwrap();
        until(additional,|s|matches!(s.state(),terminal::State::Exited(0)) && s.screen().contents().contains("[fixture-got:second-only]"));
        assert!(!deck.tabs[0].session.as_ref().unwrap().screen().contents().contains("[fixture-got:"));
        deck.close_tab(1);
        assert_eq!(deck.tabs[deck.selected].id,original);
        assert!(deck.tabs[0].running());
    }

    #[test]
    fn additional_tab_names_avoid_existing_titles_and_stop_at_the_limit(){
        let d=Device{alias:"fixture".into(),name:"Fixture".into(),host:"192.0.2.1".into(),user:"test".into(),..Default::default()};
        let mut deck=TerminalUi::default();
        deck.tabs.push(TerminalTab{name:"Fixture".into(),..Default::default()});
        deck.tabs.push(TerminalTab{name:"fixture (2)".into(),..Default::default()});
        assert_eq!(deck.unique_title("Fixture",None),"Fixture (3)");
        assert_eq!(deck.unique_title("Fixture",Some(0)),"Fixture");
        deck.tabs.resize_with(MAX_TABS,TerminalTab::default);
        assert!(!deck.has_tab_space());
        deck.start_with(&d,Route::Saved,false,true,|_|panic!("over-limit session spawned"));
        assert_eq!(deck.tabs.len(),MAX_TABS);
        assert!(deck.notice.contains("Close a tab"));
    }

    #[test]
    fn opening_preview_caps_tabs_without_dropping_existing_ones() {
        let mut deck=TerminalUi::default();
        for i in 0..=MAX_TABS{
            let d=Device{alias:format!("host-{i}"),host:format!("192.0.2.{}",i+1),user:"test".into(),..Default::default()};
            deck.start(&d,Route::Saved,true);
        }
        assert_eq!(deck.tabs.len(),MAX_TABS);
        assert!(deck.notice.contains("Close a tab"));
    }

    #[test]
    fn demo_has_no_child_and_shows_synthetic_output() {
        let mut t=TerminalUi::default();
        let d=Device{alias:"sample".into(), host:"192.0.2.13".into(),user:"sample".into(),..Default::default()};
        t.start(&d,Route::Saved,true);
        assert!(t.tabs[0].demo.as_ref().unwrap().screen().contents().contains("no SSH connection"));
        assert!(t.tabs[0].session.is_none());
    }
}
