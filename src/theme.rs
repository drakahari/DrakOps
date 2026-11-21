//! Palette import and contrast-aware native roles; never changes host settings.
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path, sync::OnceLock};
#[derive(Clone, Debug)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub group: String,
    pub light: bool,
    pub roles: Roles,
}
#[derive(Clone, Copy, Debug)]
pub struct Roles {
    pub background: Color32,
    pub panel: Color32,
    pub input: Color32,
    pub control: Color32,
    pub hover: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub border: Color32,
    pub success: Color32,
    pub error: Color32,
}
fn rgb(s: &str) -> Color32 {
    let s = s.trim_start_matches('#');
    Color32::from_rgb(
        u8::from_str_radix(&s[..2], 16).unwrap(),
        u8::from_str_radix(&s[2..4], 16).unwrap(),
        u8::from_str_radix(&s[4..6], 16).unwrap(),
    )
}
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| ((x as f32) * (1. - t) + (y as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let l = |c: Color32| {
        let f = |n: u8| {
            let v = n as f32 / 255.;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    };
    let x = l(a);
    let y = l(b);
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}
fn readable(color: Color32, bgs: &[Color32], min: f32) -> Color32 {
    let score = |c| bgs.iter().map(|b| contrast(c, *b)).fold(f32::MAX, f32::min);
    let target = if score(Color32::BLACK) > score(Color32::WHITE) {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    for i in 0..=100 {
        let c = mix(color, target, i as f32 / 100.);
        if score(c) >= min {
            return c;
        }
    }
    target
}
fn derive(bg: &str, fg: &str, muted: &str, accent: &str) -> Roles {
    let background = rgb(bg);
    let fg = rgb(fg);
    let panel = mix(background, fg, 0.035);
    let control = mix(background, fg, 0.09);
    let hover = mix(background, fg, 0.14);
    let input = background;
    let bgs = [background, panel, control, hover];
    let text = readable(fg, &bgs, 7.);
    let accent = readable(rgb(accent), &bgs, 4.5);
    let on_accent = readable(Color32::WHITE, &[accent], 4.5);
    Roles {
        background,
        panel,
        input,
        control,
        hover,
        text,
        muted: readable(rgb(muted), &bgs, 4.5),
        accent,
        on_accent,
        border: readable(rgb(muted), &bgs, 3.),
        success: readable(rgb("#16804a"), &bgs, 4.5),
        error: readable(rgb("#b62538"), &bgs, 4.5),
    }
}
pub fn inventory() -> &'static [Theme] {
    static ALL: OnceLock<Vec<Theme>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut themes = vec![Theme {
            id: "lab-navy".into(),
            name: "Learning Lab Navy".into(),
            group: "Learning Lab".into(),
            light: false,
            roles: derive("#0e1526", "#ebeffa", "#a5b4cd", "#b8a5ff"),
        }];
        let stock: serde_json::Value =
            serde_json::from_str(include_str!("../assets/themes/dlms-stock.json")).unwrap();
        for (id, p) in stock["palettes"].as_object().unwrap() {
            themes.push(Theme {
                id: format!("dlms-{id}"),
                name: stock["names"][id].as_str().unwrap().into(),
                group: "DLMS".into(),
                light: p["scheme"] == "light",
                roles: derive(
                    p["body_base"].as_str().unwrap(),
                    p["page"].as_str().unwrap(),
                    p["muted"].as_str().unwrap(),
                    p["accent_text"].as_str().unwrap(),
                ),
            })
        }
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/themes/omarchy-manifest.json")).unwrap();
        for t in manifest["themes"].as_array().unwrap() {
            let p = &t["palette"];
            let id = t["upstream_id"].as_str().unwrap();
            themes.push(Theme {
                id: format!("omarchy-{id}"),
                name: id.to_string(),
                group: "Omarchy".into(),
                light: p["mode"] == "light",
                roles: derive(
                    p["background"].as_str().unwrap(),
                    p["foreground"].as_str().unwrap(),
                    p["muted"].as_str().unwrap(),
                    p["accent"].as_str().unwrap(),
                ),
            });
        }
        themes
    })
}
pub fn find(id: &str) -> &'static Theme {
    inventory()
        .iter()
        .find(|t| t.id == id)
        .unwrap_or(&inventory()[0])
}
pub fn roles(ctx: &egui::Context) -> Roles {
    ctx.data(|d| d.get_temp::<Roles>(egui::Id::new("native-theme-roles")))
        .unwrap_or(inventory()[0].roles)
}
fn pair_widget(w: &mut egui::style::WidgetVisuals, fill: Color32, preferred_ink: Color32) {
    w.bg_fill = fill;
    w.weak_bg_fill = fill;
    w.fg_stroke.color = readable(preferred_ink, &[fill], 4.5);
}

fn primary_visuals(v: &mut egui::Visuals, r: Roles) {
    pair_widget(&mut v.widgets.inactive, r.accent, r.on_accent);
    pair_widget(&mut v.widgets.hovered, mix(r.accent, r.on_accent, 0.08), r.on_accent);
    pair_widget(&mut v.widgets.active, mix(r.accent, r.on_accent, 0.14), r.on_accent);
    pair_widget(&mut v.widgets.open, mix(r.accent, r.on_accent, 0.08), r.on_accent);
    pair_widget(&mut v.widgets.noninteractive, r.accent, r.on_accent);
    v.widgets.hovered.bg_stroke.color = r.on_accent;
    v.widgets.active.bg_stroke.color = r.on_accent;
}

pub fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool, size: egui::Vec2) -> egui::Response {
    let r = roles(ui.ctx());
    ui.scope(|ui| {
        if enabled && ui.is_enabled() {
            primary_visuals(ui.visuals_mut(), r);
            ui.add(egui::Button::new(label).min_size(size))
        } else {
            // Keep a readable, neutral disabled control rather than an active-looking accent.
            ui.add_enabled(false, egui::Button::new(egui::RichText::new(label).color(r.muted)).min_size(size))
        }
    }).inner
}

fn themed_visuals(t: &Theme) -> egui::Visuals {
    let r = t.roles;
    let mut v = if t.light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    v.panel_fill = r.background;
    v.window_fill = r.panel;
    v.extreme_bg_color = r.input;
    v.faint_bg_color = r.panel;
    v.override_text_color = None;
    v.weak_text_color = Some(r.muted);
    v.hyperlink_color = r.accent;
    v.error_fg_color = r.error;
    v.warn_fg_color = r.error;
    v.window_stroke = egui::Stroke::new(1_f32, r.border);
    v.selection.bg_fill = r.accent;
    v.selection.stroke = egui::Stroke::new(2_f32, r.on_accent);
    v.text_cursor.stroke = egui::Stroke::new(2_f32, r.accent);
    v.disabled_alpha = 1.;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.bg_fill = r.control;
        w.weak_bg_fill = r.control;
        w.bg_stroke = egui::Stroke::new(1_f32, r.border);
        w.fg_stroke = egui::Stroke::new(1_f32, r.text);
    }
    v.widgets.noninteractive.bg_fill = r.panel;
    v.widgets.noninteractive.weak_bg_fill = r.panel;
    v.widgets.noninteractive.fg_stroke.color = r.text;
    v.widgets.hovered.bg_fill = r.hover;
    v.widgets.hovered.weak_bg_fill = r.hover;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(2_f32, r.accent);
    v.widgets.active.bg_stroke = egui::Stroke::new(2_f32, r.accent);
    v.widgets.open.bg_stroke = egui::Stroke::new(2_f32, r.accent);
    let preferred=r.text;
    for w in [&mut v.widgets.noninteractive,&mut v.widgets.inactive,&mut v.widgets.hovered,&mut v.widgets.active,&mut v.widgets.open] {
        w.fg_stroke.color=readable(preferred,&[w.bg_fill,w.weak_bg_fill],4.5);
    }
    v
}
pub fn connect_split(ui:&mut egui::Ui)->(egui::Response,egui::Response) {
    let colors=roles(ui.ctx());
    ui.horizontal(|ui|{
        ui.spacing_mut().item_spacing.x=0.;
        primary_visuals(ui.visuals_mut(),colors);
        let main=ui.add(egui::Button::new("Connect").min_size(egui::vec2(110.,33.))
            .corner_radius(egui::CornerRadius{nw:6,sw:6,ne:0,se:0}));
        let arrow=ui.add(egui::Button::new("").min_size(egui::vec2(36.,33.))
            .corner_radius(egui::CornerRadius{nw:0,sw:0,ne:6,se:6}));
        let ink=ui.style().interact(&arrow).text_color();
        let p=arrow.rect.center();
        let tip=p+egui::vec2(0.,2.);
        let stroke=egui::Stroke::new(2_f32,ink);
        ui.painter().line_segment([p+egui::vec2(-4.,-2.),tip],stroke);
        ui.painter().line_segment([tip,p+egui::vec2(4.,-2.)],stroke);
        ui.painter().line_segment([egui::pos2(arrow.rect.left()+1.,p.y-8.),
            egui::pos2(arrow.rect.left()+1.,p.y+8.)],egui::Stroke::new(1_f32,ink));
        arrow.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::Button,true,"SSH connection options"));
        (main,arrow.on_hover_text("SSH connection options"))
    }).inner
}

pub fn apply(ctx: &egui::Context, id: &str) {
    let t=find(id);
    let r=t.roles;
    ctx.set_visuals(themed_visuals(t));
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("native-theme-roles"), r));
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub theme: String,
    pub text_scale: f32,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            theme: "lab-navy".into(),
            text_scale: 1.,
        }
    }
}
impl Preferences {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 4096 {
            return Err("Appearance settings exceed size limit; original retained".into());
        }
        let mut p: Self = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if p.version != 1 || ![1., 1.15, 1.25, 1.5, 2.].contains(&p.text_scale) {
            return Err("Unsupported appearance settings; original retained".into());
        }
        p.theme = find(&p.theme).id.clone();
        Ok(p)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if self.version != 1
            || find(&self.theme).id != self.theme
            || ![1., 1.15, 1.25, 1.5, 2.].contains(&self.text_scale)
        {
            return Err("Invalid appearance selection".into());
        }
        let parent = path.parent().ok_or("Missing preferences directory")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut f = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut f, self).map_err(|e| e.to_string())?;
        f.flush()
            .and_then(|_| f.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        f.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;

    #[test]
    fn enabled_and_disabled_button_pairs_are_readable_in_all_themes() {
        assert_eq!(inventory().len(),28);
        for t in inventory() {
            let normal=themed_visuals(t);
            let mut primary=normal.clone();
            primary_visuals(&mut primary,t.roles);
            for (name, v) in [("normal", &normal), ("primary", &primary)] {
                for (state, w) in [("inactive",&v.widgets.inactive),("hovered",&v.widgets.hovered),
                    ("pressed / focused",&v.widgets.active),("open",&v.widgets.open),
                    ("non-interactive",&v.widgets.noninteractive)] {
                    assert!(contrast(w.text_color(),w.weak_bg_fill)>=4.5,
                        "{} {name} {state}: {:?} on {:?}",t.id,w.text_color(),w.weak_bg_fill);
                }
            }
            assert!(contrast(normal.selection.stroke.color,normal.selection.bg_fill)>=4.5,"{} selected",t.id);
            assert!(contrast(t.roles.muted,normal.widgets.inactive.weak_bg_fill)>=4.5,"{} disabled",t.id);
        }
    }

    #[test]
    fn split_connect_uses_two_joined_independent_click_targets(){
        let ctx=egui::Context::default();
        let frame=|events|{
            let mut layout=None;
            let _=ctx.run(egui::RawInput{events,..Default::default()},|ctx|egui::CentralPanel::default().show(ctx,|ui|{
                let (main,arrow)=connect_split(ui);
                layout=Some((main.rect,arrow.rect,main.clicked(),arrow.clicked()));
            }).inner);
            layout.unwrap()
        };
        let (main,arrow,_,_)=frame(vec![]);
        assert!((main.right()-arrow.left()).abs()<=1.);
        assert_eq!(main.height(),arrow.height());
        assert!(arrow.width()>=36.);
        let mouse=|pos,pressed|egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed,modifiers:egui::Modifiers::NONE};
        let p=arrow.center();
        frame(vec![egui::Event::PointerMoved(p),mouse(p,true)]);
        let (_,_,main_clicked,arrow_clicked)=frame(vec![mouse(p,false)]);
        assert!(!main_clicked && arrow_clicked);
    }

    #[test]
    fn ethereal_selected_and_primary_use_distinct_ink() {
        let t=find("omarchy-ethereal");
        assert_eq!(t.id,"omarchy-ethereal");
        let ctx=egui::Context::default();
        apply(&ctx,&t.id);
        let _=ctx.run(egui::RawInput::default(),|ctx|{
            egui::CentralPanel::default().show(ctx,|ui| {
                let selected=ui.add(egui::Button::selectable(true,"fedora"));
                let v=ui.style().interact_selectable(&selected,true);
                assert_eq!(v.text_color(),t.roles.on_accent);
                assert!(contrast(v.text_color(),v.weak_bg_fill)>=4.5);
                let r=primary_button(ui,"Connect",true,egui::vec2(110.,33.));
                assert!(r.enabled());
                let disabled=primary_button(ui,"Connect",false,egui::vec2(110.,33.));
                assert!(!disabled.enabled());
            });
        });
    }
}