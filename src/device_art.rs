use super::{DeviceKind, egui, theme};
use egui::{Color32, Rect, Stroke, StrokeKind, Vec2};

pub fn illustration(ui: &mut egui::Ui, kind: DeviceKind, size: f32) {
    let (bounds, _) = ui.allocate_exact_size(egui::vec2(size * 1.22, size), egui::Sense::hover());
    let roles = theme::roles(ui.ctx());
    let p = ui.painter();
    let pt = |x: f32, y: f32| bounds.min + Vec2::new(x*bounds.width()/100., y*bounds.height()/82.);
    let rect = |x,y,w,h| Rect::from_min_max(pt(x,y),pt(x+w,y+h));
    let outline = Stroke::new((size * 0.043).clamp(1.4,3.),roles.text);
    let fill = roles.panel;
    let accent = roles.accent;
    let draw = |x,y,w,h,r: f32,ink: Color32| {p.rect(rect(x,y,w,h),r,ink,outline,StrokeKind::Inside);};
    let line = |x,y,x2,y2| {p.line_segment([pt(x,y),pt(x2,y2)],outline);};
    p.rect_filled(bounds,12.,roles.background);
    p.circle_filled(pt(52.,40.),size*0.37,accent.gamma_multiply(0.17));
    match kind {
        DeviceKind::Desktop => {
            draw(15.,17.,52.,35.,5.,fill);
            p.rect_filled(rect(21.,23.,40.,22.),2.,accent.gamma_multiply(0.45));
            line(41.,51.,41.,64.);line(28.,64.,55.,64.);
            draw(73.,20.,14.,43.,3.,fill);
            p.circle_filled(pt(80.,53.),size*0.038,accent);
            line(77.,28.,83.,28.);
        }
        DeviceKind::Laptop => {
            draw(23.,17.,54.,38.,5.,fill);
            p.rect_filled(rect(29.,23.,42.,25.),2.,accent.gamma_multiply(0.46));
            draw(15.,55.,70.,9.,4.,fill);
            line(44.,59.,56.,59.);
        }
        DeviceKind::Server => {
            for y in [13.,33.,53.] {
                draw(25.,y,50.,16.,4.,fill);
                p.circle_filled(pt(65.,y+8.),size*0.032,accent);
                line(32.,y+7.,49.,y+7.);
            }
        }
        DeviceKind::Iot => {
            draw(31.,31.,38.,34.,6.,fill);
            draw(43.,41.,14.,14.,3.,accent.gamma_multiply(0.51));
            for x in [37.,50.,63.] {line(x,25.,x,31.);line(x,65.,x,71.);}
            p.add(egui::Shape::line([pt(38.,21.),pt(50.,14.),pt(62.,21.)].to_vec(),outline));
        }
        DeviceKind::Unspecified => {
            draw(24.,20.,52.,42.,8.,fill);
            p.line_segment([pt(37.,39.),pt(62.,39.)],Stroke::new(outline.width,accent));
            p.circle_filled(pt(50.,51.),size*0.035,accent);
        }
    }
}
