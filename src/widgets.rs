//! Small custom-painted widgets that give the app its look.

use crate::theme::{mix, with_alpha, Palette};
use egui::{pos2, vec2, Align2, Color32, FontFamily, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui};

pub fn ui_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("ui-bold".into()))
}

pub fn display_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

pub fn mono_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Square icon button.
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str, pal: &Palette, active: bool) -> Response {
    icon_button_sized(ui, icon, tip, pal, active, 30.0)
}

pub fn icon_button_sized(ui: &mut Ui, icon: &str, tip: &str, pal: &Palette, active: bool, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || active, 0.12);
    let bg = if active { with_alpha(pal.accent, 40) } else { with_alpha(pal.text, (t * 18.0) as u8) };
    ui.painter().rect_filled(rect, 7.0, bg);
    let col = if active { pal.accent } else { mix(pal.subtext, pal.bright, t) };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, ui_font(size * 0.46), col);
    if !tip.is_empty() {
        resp.on_hover_text(tip)
    } else {
        resp
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum BtnKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

pub fn button(ui: &mut Ui, icon: &str, label: &str, pal: &Palette, kind: BtnKind) -> Response {
    let font = ui_font(13.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), pal.text);
    let icon_w = if icon.is_empty() { 0.0 } else { 20.0 };
    let w = galley.size().x + icon_w + 26.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let hov = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let pressed = resp.is_pointer_button_down_on();
    let (bg, fg, stroke) = match kind {
        BtnKind::Primary => {
            let b = if pressed { mix(pal.accent, Color32::BLACK, 0.12) } else { mix(pal.accent, pal.bright, hov * 0.12) };
            (b, pal.on_accent, Stroke::NONE)
        }
        BtnKind::Secondary => (mix(pal.surface, pal.overlay, hov), pal.text, Stroke::new(1.0, pal.border)),
        BtnKind::Ghost => (with_alpha(pal.text, (hov * 16.0) as u8), mix(pal.subtext, pal.bright, hov), Stroke::NONE),
        BtnKind::Danger => (mix(with_alpha(pal.red, 30), with_alpha(pal.red, 60), hov), pal.red, Stroke::new(1.0, with_alpha(pal.red, 90))),
    };
    let p = ui.painter();
    if kind == BtnKind::Primary {
        p.rect_filled(rect.translate(vec2(0.0, 2.0)), 9.0, with_alpha(pal.accent, (30.0 + hov * 30.0) as u8));
    }
    p.rect_filled(rect, 8.0, bg);
    if stroke != Stroke::NONE {
        p.rect_stroke(rect, 8.0, stroke, StrokeKind::Inside);
    }
    let mut x = rect.min.x + 13.0;
    if !icon.is_empty() {
        p.text(pos2(x + 7.0, rect.center().y), Align2::CENTER_CENTER, icon, ui_font(13.0), fg);
        x += icon_w;
    }
    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    resp
}

/// Animated segmented control. Returns true if the selection changed.
pub fn segmented(ui: &mut Ui, id: &str, items: &[(&str, &str, Option<String>)], selected: &mut usize, pal: &Palette) -> bool {
    let font = ui_font(13.5);
    let widths: Vec<f32> = items
        .iter()
        .map(|(_, l, b)| {
            let g = ui.painter().layout_no_wrap(l.to_string(), font.clone(), pal.text);
            g.size().x + 52.0 + b.as_ref().map_or(0.0, |b| b.len() as f32 * 7.0 + 14.0)
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 36.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, 11.0, mix(pal.mantle, pal.crust, 0.5));
    p.rect_stroke(rect, 11.0, Stroke::new(1.0, with_alpha(pal.border, 140)), StrokeKind::Inside);
    let mut changed = false;
    let mut x = rect.min.x + 4.0;
    let mut rects = vec![];
    for (i, w) in widths.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.min.y + 4.0), vec2(*w, 28.0));
        rects.push(r);
        let resp = ui.interact(r, ui.id().with((id, i)), Sense::click());
        if resp.clicked() && *selected != i {
            *selected = i;
            changed = true;
        }
        if resp.hovered() && *selected != i {
            p.rect_filled(r, 8.0, with_alpha(pal.text, 10));
        }
        x += w;
    }
    // sliding highlight
    let target = rects[*selected];
    let ax = ui.ctx().animate_value_with_time(ui.id().with((id, "x")), target.min.x, 0.18);
    let aw = ui.ctx().animate_value_with_time(ui.id().with((id, "w")), target.width(), 0.18);
    let hr = Rect::from_min_size(pos2(ax, target.min.y), vec2(aw, target.height()));
    p.rect_filled(hr.translate(vec2(0.0, 1.0)), 8.0, with_alpha(Color32::BLACK, 40));
    p.rect_filled(hr, 8.0, pal.surface);
    p.rect_stroke(hr, 8.0, Stroke::new(1.0, with_alpha(pal.border, 200)), StrokeKind::Inside);
    for (i, (icon, label, badge)) in items.iter().enumerate() {
        let r = rects[i];
        let sel = i == *selected;
        let fg = if sel { pal.bright } else { pal.subtext };
        let ic = if sel { pal.accent } else { pal.dim };
        p.text(pos2(r.min.x + 20.0, r.center().y), Align2::CENTER_CENTER, *icon, ui_font(13.0), ic);
        let g = p.layout_no_wrap(label.to_string(), font.clone(), fg);
        let lw = g.size().x;
        p.galley(pos2(r.min.x + 34.0, r.center().y - g.size().y / 2.0), g, fg);
        if let Some(b) = badge {
            let bx = r.min.x + 34.0 + lw + 8.0;
            let bw = b.len() as f32 * 7.0 + 10.0;
            let br = Rect::from_min_size(pos2(bx, r.center().y - 9.0), vec2(bw, 18.0));
            p.rect_filled(br, 9.0, if sel { with_alpha(pal.accent, 45) } else { with_alpha(pal.text, 18) });
            p.text(br.center(), Align2::CENTER_CENTER, b, ui_font(11.0), if sel { pal.accent } else { pal.subtext });
        }
    }
    changed
}

pub fn chip(ui: &mut Ui, icon: &str, label: &str, selected: bool, color: Color32, pal: &Palette) -> Response {
    let font = ui_font(12.5);
    let text = if icon.is_empty() { label.to_string() } else if label.is_empty() { icon.to_string() } else { format!("{icon}  {label}") };
    let g = ui.painter().layout_no_wrap(text.clone(), font.clone(), pal.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 22.0, 26.0), Sense::click());
    let hov = resp.hovered();
    let (bg, fg, st) = if selected {
        (with_alpha(color, 40), mix(color, pal.bright, 0.25), Stroke::new(1.0, with_alpha(color, 120)))
    } else {
        (if hov { pal.overlay } else { pal.surface }, pal.subtext, Stroke::new(1.0, with_alpha(pal.border, 160)))
    };
    ui.painter().rect_filled(rect, 13.0, bg);
    ui.painter().rect_stroke(rect, 13.0, st, StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, fg);
    resp
}

pub fn section_label(ui: &mut Ui, text: &str, pal: &Palette) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text.to_uppercase()).font(ui_font(10.5)).color(pal.dim).extra_letter_spacing(1.2));
    ui.add_space(2.0);
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = ui_font(10.5);
    let g = ui.painter().layout_no_wrap(text.to_string(), font.clone(), color);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 18.0), Sense::hover());
    ui.painter().rect_filled(rect, 5.0, with_alpha(color, 32));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, color);
    resp
}

/// Text field with a leading icon and rounded frame.
pub fn search_field(ui: &mut Ui, text: &mut String, hint: &str, icon: &str, pal: &Palette, width: f32, id: &str) -> Response {
    let h = 34.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let id = ui.id().with(id);
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let p = ui.painter().clone();
    p.rect_filled(rect, 9.0, pal.base);
    p.rect_stroke(rect, 9.0, Stroke::new(1.0, if focused { with_alpha(pal.accent, 180) } else { pal.border }), StrokeKind::Inside);
    if focused {
        p.rect_stroke(rect.expand(2.5), 11.0, Stroke::new(2.5, with_alpha(pal.accent, 40)), StrokeKind::Inside);
    }
    p.text(pos2(rect.min.x + 16.0, rect.center().y), Align2::CENTER_CENTER, icon, ui_font(12.5), pal.dim);
    let inner = Rect::from_min_max(pos2(rect.min.x + 32.0, rect.min.y), pos2(rect.max.x - 8.0, rect.max.y));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(egui::RichText::new(hint).color(pal.dim))
            .frame(egui::Frame::NONE)
            .font(ui_font(13.5))
            .desired_width(inner.width()),
    )
}

/// Deterministic accent color for a string (used for paper spines).
pub fn color_for(s: &str, pal: &Palette) -> Color32 {
    let cols = [pal.accent, pal.blue, pal.green, pal.magenta, pal.cyan, pal.orange, pal.yellow, pal.red];
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    cols[(h as usize) % cols.len()]
}

pub fn draw_spinner(ui: &Ui, center: egui::Pos2, r: f32, color: Color32) {
    let t = ui.input(|i| i.time) as f32;
    let n = 24;
    let start = t * 5.0;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let a = start + i as f32 / n as f32 * std::f32::consts::PI * 1.4;
            pos2(center.x + r * a.cos(), center.y + r * a.sin())
        })
        .collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0, color)));
    ui.ctx().request_repaint();
}

pub struct FontIdExt;
impl FontIdExt {
    pub fn serif(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("serif".into()))
    }
    pub fn serif_bold(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("serif-bold".into()))
    }
}

/// Colour for a git status letter (M, N, A, D, U, R).
pub fn git_color(c: char, pal: &Palette) -> Color32 {
    match c {
        'N' | 'A' => pal.green,
        'D' | 'U' => pal.red,
        'R' => pal.cyan,
        _ => crate::theme::mix(pal.yellow, pal.orange, 0.35),
    }
}

/// A clearly visible slider: track line, accent fill, round knob, value label.
pub fn fancy_slider(ui: &mut Ui, value: &mut f32, min: f32, max: f32, step: f32, unit: &str, pal: &Palette) -> Response {
    let w = 170.0;
    let (rect, mut resp) = ui.allocate_exact_size(vec2(w + 58.0, 26.0), Sense::click_and_drag());
    let track = Rect::from_min_max(pos2(rect.min.x + 8.0, rect.center().y - 2.0), pos2(rect.min.x + w - 8.0, rect.center().y + 2.0));
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - track.min.x) / track.width()).clamp(0.0, 1.0);
            let v = ((min + t * (max - min)) / step).round() * step;
            if (v - *value).abs() > f32::EPSILON {
                *value = v.clamp(min, max);
                resp.mark_changed();
            }
        }
    }
    if resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta.y);
        if d.abs() > 0.5 {
            *value = (*value + step * d.signum()).clamp(min, max);
            resp.mark_changed();
        }
    }
    let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let kx = track.min.x + t * track.width();
    let p = ui.painter();
    // tick marks every whole unit
    let mut v = min.ceil();
    while v <= max {
        let x = track.min.x + (v - min) / (max - min) * track.width();
        let major = (v as i32) % 2 == 0;
        p.line_segment([pos2(x, track.max.y + 4.0), pos2(x, track.max.y + if major { 8.0 } else { 6.0 })], Stroke::new(1.0, with_alpha(pal.dim, if major { 160 } else { 90 })));
        v += 1.0;
    }
    p.rect_filled(track, 2.0, mix(pal.base, pal.text, 0.18));
    p.rect_filled(Rect::from_min_max(track.min, pos2(kx, track.max.y)), 2.0, pal.accent);
    let active = resp.hovered() || resp.dragged();
    let r = if resp.dragged() { 9.0 } else if active { 8.0 } else { 7.0 };
    p.circle_filled(pos2(kx, rect.center().y + 1.5), r + 1.0, with_alpha(Color32::BLACK, 50));
    p.circle_filled(pos2(kx, rect.center().y), r, pal.accent);
    p.circle_filled(pos2(kx, rect.center().y), r * 0.42, pal.on_accent);
    if active {
        p.circle_stroke(pos2(kx, rect.center().y), r + 4.0, Stroke::new(3.0, with_alpha(pal.accent, 50)));
    }
    let label = if step < 1.0 { format!("{:.1} {unit}", value) } else { format!("{:.0} {unit}", value) };
    let lr = Rect::from_min_size(pos2(rect.min.x + w + 4.0, rect.center().y - 11.0), vec2(52.0, 22.0));
    p.rect_filled(lr, 6.0, pal.base);
    p.text(lr.center(), Align2::CENTER_CENTER, label, ui_font(12.0), pal.text);
    resp
}
