//! Pomodoro timer as a tomato: the tomato is the clock (a dark wedge runs down like a hand),
//! red while working, green (unripe) in short breaks, golden in the long break. A chime is
//! synthesized and played through the system at the end of each phase.

use crate::app::App;
use crate::icons as ic;
use crate::theme::{with_alpha, Palette};
use crate::widgets;
use egui::{pos2, vec2, Color32, Id, Pos2, Rect, Sense, Shape, Stroke};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Work,
    Short,
    Long,
}

#[derive(Default)]
pub struct Pomodoro {
    pub phase: Phase,
    pub running: bool,
    ends_at: Option<Instant>,
    /// seconds left while paused (None = full phase)
    paused_left: Option<f64>,
    /// finished work sessions in the current cycle
    pub round: u32,
    /// when the last phase ended (for the wobble "I'm waiting" animation)
    finished_at: Option<Instant>,
}

impl Pomodoro {
    pub fn phase_secs(&self, s: &crate::app::Settings) -> f64 {
        60.0 * match self.phase {
            Phase::Work => s.pomo_work,
            Phase::Short => s.pomo_short,
            Phase::Long => s.pomo_long,
        } as f64
    }

    pub fn left(&self, s: &crate::app::Settings) -> f64 {
        match (self.running, self.ends_at) {
            (true, Some(e)) => e.saturating_duration_since(Instant::now()).as_secs_f64(),
            _ => self.paused_left.unwrap_or_else(|| self.phase_secs(s)),
        }
    }

    /// 1.0 = full, 0.0 = done
    pub fn fraction_left(&self, s: &crate::app::Settings) -> f32 {
        (self.left(s) / self.phase_secs(s).max(1.0)) as f32
    }

    pub fn toggle(&mut self, s: &crate::app::Settings) {
        if self.running {
            self.paused_left = Some(self.left(s));
            self.running = false;
            self.ends_at = None;
        } else {
            let left = self.left(s);
            self.ends_at = Some(Instant::now() + Duration::from_secs_f64(left));
            self.running = true;
            self.finished_at = None;
        }
    }

    pub fn reset(&mut self) {
        *self = Pomodoro::default();
    }

    fn start_phase(&mut self, p: Phase, s: &crate::app::Settings, run: bool) {
        self.phase = p;
        self.paused_left = None;
        self.running = run;
        self.ends_at = run.then(|| Instant::now() + Duration::from_secs_f64(self.phase_secs(s)));
    }

    /// Skip to the next phase (counts a skipped work session as not done).
    pub fn skip(&mut self, s: &crate::app::Settings) {
        let next = match self.phase {
            Phase::Work => Phase::Short,
            _ => Phase::Work,
        };
        self.start_phase(next, s, false);
    }

    pub fn label(&self, s: &crate::app::Settings) -> String {
        let l = self.left(s).ceil() as u64;
        format!("{:02}:{:02}", l / 60, l % 60)
    }

    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::Work => tr!("Fokus" | "Focus"),
            Phase::Short => tr!("Kurze Pause" | "Short break"),
            Phase::Long => tr!("Lange Pause" | "Long break"),
        }
    }

    pub fn waiting(&self) -> bool {
        self.finished_at.is_some() && !self.running
    }
}

fn phase_color(p: Phase) -> Color32 {
    match p {
        Phase::Work => Color32::from_rgb(0xe5, 0x48, 0x4d),
        Phase::Short => Color32::from_rgb(0x5f, 0xb5, 0x5a),
        Phase::Long => Color32::from_rgb(0xf0, 0xa8, 0x2e),
    }
}

/// Paint a tomato. `left` = remaining fraction (the colored wedge), `t` = time for animation.
pub fn paint_tomato(p: &egui::Painter, c: Pos2, r: f32, left: f32, phase: Phase, running: bool, waiting: bool, t: f64) {
    let base = phase_color(phase);
    // breathing while running, wobbling while waiting
    let s = if running { 1.0 + 0.018 * (t * 2.2).sin() as f32 } else { 1.0 };
    let c = if waiting { pos2(c.x + 2.5 * (t * 9.0).sin() as f32 * ((t * 1.3).sin().max(0.0)) as f32, c.y) } else { c };
    let rx = r * 1.08 * s;
    let ry = r * 0.94 * s;
    // shadow
    p.add(Shape::ellipse_filled(c + vec2(0.0, ry * 0.9), vec2(rx * 0.75, ry * 0.18), with_alpha(Color32::BLACK, 45)));
    // body: pale, then the remaining part as a clock wedge from 12 o'clock
    let pale = base.lerp_to_gamma(Color32::from_gray(40), 0.62);
    p.add(Shape::ellipse_filled(c, vec2(rx, ry), pale));
    if left > 0.0 {
        let steps = (120.0 * left).ceil().max(2.0) as usize;
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(c, base);
        for i in 0..=steps {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * left * i as f32 / steps as f32;
            mesh.colored_vertex(pos2(c.x + rx * a.cos(), c.y + ry * a.sin()), base);
        }
        for i in 1..=steps as u32 {
            mesh.add_triangle(0, i, i + 1);
        }
        p.add(Shape::mesh(mesh));
    }
    // ribs for a tomato look
    let rib = with_alpha(Color32::BLACK, 28);
    for k in [-0.45f32, 0.45] {
        p.add(Shape::ellipse_stroke(c + vec2(k * rx * 0.55, 0.0), vec2(rx * 0.32, ry * 0.92), Stroke::new(1.0, rib)));
    }
    // shine
    p.add(Shape::ellipse_filled(c + vec2(-rx * 0.42, -ry * 0.38), vec2(rx * 0.2, ry * 0.12), with_alpha(Color32::WHITE, 70)));
    // calyx: a five-pointed green star with drooping leaves, and a stem
    let green = Color32::from_rgb(0x3f, 0x9a, 0x46);
    let dark = Color32::from_rgb(0x2b, 0x70, 0x31);
    let top = c + vec2(0.0, -ry * 0.78);
    for i in 0..5 {
        // spread over the top of the fruit, leaves hang down a little
        let a = std::f32::consts::PI * (1.05 + 0.225 * i as f32);
        let len = r * if i % 2 == 0 { 0.5 } else { 0.42 };
        let tip = top + vec2(a.cos() * len, a.sin() * len * 0.35 + len * 0.42);
        let dir = (tip - top).normalized();
        let side = vec2(-dir.y, dir.x) * r * 0.1;
        let mid = top + (tip - top) * 0.35;
        p.add(Shape::convex_polygon(vec![top, mid + side, tip, mid - side], green, Stroke::new(0.8, dark)));
    }
    p.add(Shape::circle_filled(top, r * 0.09, dark));
    p.add(Shape::line(vec![top, top + vec2(r * 0.04, -r * 0.18), top + vec2(r * 0.12, -r * 0.3)], Stroke::new(r * 0.09, dark)));
}

// ───────────────────────────── chime ─────────────────────────────

/// Which chime to play.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chime {
    /// end of a phase: a short bell (E6 G6 C7)
    Short,
    /// the long break starts: a music-box arpeggio over a warm chord with an echo tail
    Long,
}

/// A bell tone with a few partials and exponential decay, added into `out`.
fn bell(out: &mut [f32], rate: f32, f: f32, start: f32, decay: f32, amp: f32) {
    let s0 = (start * rate) as usize;
    // partials of a soft bell / celesta: (ratio, level, extra decay)
    let partials = [(1.0f32, 1.0f32, 1.0f32), (2.0, 0.28, 1.6), (3.0, 0.1, 2.4), (4.16, 0.05, 3.2)];
    for (i, o) in out.iter_mut().enumerate().skip(s0) {
        let t = (i - s0) as f32 / rate;
        let attack = 1.0 - (-t * 300.0).exp();
        let mut v = 0.0;
        for (r, l, d) in partials {
            v += l * (std::f32::consts::TAU * f * r * t).sin() * (-t * decay * d).exp();
        }
        *o += amp * attack * v;
    }
}

/// A soft pad note (slow attack, slow release) for the chord under the long chime.
fn pad(out: &mut [f32], rate: f32, f: f32, start: f32, len: f32, amp: f32) {
    let s0 = (start * rate) as usize;
    for (i, o) in out.iter_mut().enumerate().skip(s0) {
        let t = (i - s0) as f32 / rate;
        let env = (t / 0.35).min(1.0) * if t < len { 1.0 } else { (-(t - len) * 2.5).exp() };
        // slight detune for warmth
        let v = (std::f32::consts::TAU * f * t).sin() + 0.6 * (std::f32::consts::TAU * f * 1.003 * t).sin() + 0.15 * (std::f32::consts::TAU * f * 2.0 * t).sin();
        *o += amp * env * v;
    }
}

fn synth(kind: Chime, rate: u32) -> Vec<f32> {
    let r = rate as f32;
    match kind {
        Chime::Short => {
            let mut s = vec![0f32; (r * 1.6) as usize];
            for (f, start) in [(1318.5f32, 0.0f32), (1568.0, 0.12), (2093.0, 0.24)] {
                bell(&mut s, r, f, start, 3.2, 0.6);
            }
            s
        }
        Chime::Long => {
            let mut s = vec![0f32; (r * 4.2) as usize];
            // warm C major chord swelling underneath
            for f in [261.63f32, 329.63, 392.0, 523.25] {
                pad(&mut s, r, f, 0.0, 1.4, 0.07);
            }
            // music-box arpeggio up and a final high sparkle: C6 E6 G6 C7 – E7 G7 C8 (soft)
            let notes = [(1046.5f32, 0.0f32, 0.55f32), (1318.5, 0.16, 0.5), (1568.0, 0.32, 0.5), (2093.0, 0.48, 0.55), (2637.0, 0.78, 0.25), (3136.0, 0.9, 0.2), (4186.0, 1.02, 0.14)];
            for (f, start, amp) in notes {
                bell(&mut s, r, f, start, 1.8, amp);
            }
            // echo tail: two soft feedback delays for a little space
            for (delay, gain) in [(0.23f32, 0.32f32), (0.37, 0.22)] {
                let d = (delay * r) as usize;
                for i in d..s.len() {
                    s[i] += s[i - d] * gain;
                }
            }
            s
        }
    }
}

/// The chime as 16-bit mono WAV.
fn chime_wav(kind: Chime) -> Vec<u8> {
    let rate = 44_100u32;
    let mut samples = synth(kind, rate);
    // normalize and fade out the last 150 ms (no click at the end)
    let peak = samples.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let fade = (0.15 * rate as f32) as usize;
    let n = samples.len();
    for (i, v) in samples.iter_mut().enumerate() {
        *v = *v / peak * 0.5 * ((n - i) as f32 / fade as f32).min(1.0);
    }
    let mut wav = Vec::with_capacity(44 + n * 2);
    let data_len = (n * 2) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for v in samples {
        wav.extend_from_slice(&((v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    wav
}

/// Play a chime in the background with the system's audio tools.
pub fn play_chime(kind: Chime) {
    std::thread::spawn(move || {
        // versioned name, so a changed sound replaces the cached file
        let name = match kind {
            Chime::Short => "nedit-chime-2.wav",
            Chime::Long => "nedit-chime-long-2.wav",
        };
        let f = std::env::temp_dir().join(name);
        if !f.exists() && std::fs::write(&f, chime_wav(kind)).is_err() {
            return;
        }
        #[cfg(target_os = "macos")]
        let _ = crate::platform::cmd("afplay").arg(&f).status();
        #[cfg(windows)]
        let _ = crate::platform::cmd("powershell")
            .args(["-NoProfile", "-Command", &format!("(New-Object Media.SoundPlayer '{}').PlaySync()", f.display())])
            .status();
        #[cfg(not(any(target_os = "macos", windows)))]
        for player in ["pw-play", "paplay", "aplay"] {
            if crate::platform::has_tool(player) {
                let _ = crate::platform::cmd(player).arg(&f).status();
                break;
            }
        }
    });
}

// ───────────────────────────── App integration ─────────────────────────────

impl App {
    /// Advance the timer; called every frame.
    pub fn pomodoro_tick(&mut self, ctx: &egui::Context, now: f64) {
        if !self.pomo.running {
            if self.pomo.waiting() {
                ctx.request_repaint_after(Duration::from_millis(60));
            }
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(if self.settings.pomo_float || self.tab == crate::app::Tab::Shelf { 50 } else { 500 }));
        if self.pomo.left(&self.settings) > 0.0 {
            return;
        }
        // phase finished
        let s = self.settings.clone();
        let (next, run, msg) = match self.pomo.phase {
            Phase::Work => {
                self.pomo.round += 1;
                let day = crate::plan::today().to_string();
                *self.plan.pomodoros.entry(day).or_insert(0) += 1;
                self.plan.save();
                if self.pomo.round >= s.pomo_rounds {
                    self.pomo.round = 0;
                    (Phase::Long, true, tr!("Pomodoro geschafft – lange Pause!" | "Pomodoro done – long break!"))
                } else {
                    (Phase::Short, true, tr!("Pomodoro geschafft – kurze Pause" | "Pomodoro done – short break"))
                }
            }
            _ => (Phase::Work, false, tr!("Pause vorbei – Tomate antippen für die nächste Runde" | "Break over – tap the tomato for the next round")),
        };
        if s.pomo_sound {
            play_chime(if next == Phase::Long { Chime::Long } else { Chime::Short });
        }
        self.pomo.start_phase(next, &s, run);
        self.pomo.finished_at = (!run).then(Instant::now);
        let c = phase_color(next);
        self.toast(ic::CLOCK, msg, c, now);
    }

    /// Tomato card in the plan header.
    pub fn pomodoro_card(&mut self, ui: &mut egui::Ui, pal: &Palette, now: f64) {
        egui::Frame::new()
            .fill(crate::theme::mix(pal.surface, pal.base, 0.3))
            .stroke(Stroke::new(1.0, with_alpha(pal.border, 110)))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_height(48.0);
                ui.horizontal_centered(|ui| {
                    let (r, resp) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::click());
                    let resp = resp.on_hover_text(if self.pomo.running { tr!("Pausieren" | "Pause") } else { tr!("Starten" | "Start") });
                    paint_tomato(ui.painter(), r.center() + vec2(0.0, 2.0), 19.0, self.pomo.fraction_left(&self.settings), self.pomo.phase, self.pomo.running, self.pomo.waiting(), now);
                    if resp.clicked() {
                        self.pomo.toggle(&self.settings);
                    }
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(self.pomo.phase_name().to_uppercase()).font(widgets::ui_font(10.0)).color(phase_color(self.pomo.phase)).extra_letter_spacing(1.0));
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(self.pomo.label(&self.settings)).font(widgets::mono_font(15.0)).color(pal.bright));
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if widgets::icon_button_sized(ui, if self.pomo.running { ic::STOP } else { ic::PLAY }, if self.pomo.running { tr!("Pausieren" | "Pause") } else { tr!("Starten" | "Start") }, pal, false, 22.0).clicked() {
                                self.pomo.toggle(&self.settings);
                            }
                            if widgets::icon_button_sized(ui, ic::STEP, tr!("Überspringen" | "Skip"), pal, false, 22.0).clicked() {
                                self.pomo.skip(&self.settings);
                            }
                            if widgets::icon_button_sized(ui, ic::UNDO, tr!("Zurücksetzen" | "Reset"), pal, false, 22.0).clicked() {
                                self.pomo.reset();
                            }
                        });
                        // rounds as little tomatoes + today's count
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            for k in 0..self.settings.pomo_rounds {
                                let (r, _) = ui.allocate_exact_size(vec2(9.0, 9.0), Sense::hover());
                                let done = k < self.pomo.round;
                                ui.painter().circle_filled(r.center(), 3.6, if done { phase_color(Phase::Work) } else { with_alpha(pal.text, 40) });
                            }
                            let today = self.plan.pomodoros.get(&crate::plan::today().to_string()).copied().unwrap_or(0);
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(trf!("{today} heute" | "{today} today")).font(widgets::ui_font(10.5)).color(pal.dim));
                        });
                    });
                });
            });
    }

    /// The floating, draggable tomato.
    pub fn pomodoro_float(&mut self, ctx: &egui::Context, pal: &Palette, now: f64) {
        if !self.settings.pomo_float {
            return;
        }
        // position = distance of the tomato's top left from the bottom right window corner,
        // so it stays in its corner when the window size changes (default: bottom right)
        let screen = ctx.content_rect();
        let size = vec2(92.0, 104.0);
        let off = self.settings.pomo_corner.map(|p| vec2(p[0], p[1])).unwrap_or(vec2(124.0, 150.0));
        let max = (screen.max - size).max(screen.min);
        let pos = (screen.max - off).clamp(screen.min, max);
        let area = egui::Area::new(Id::new("pomo-float")).order(egui::Order::Foreground).fixed_pos(pos);
        let resp = area.show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            if resp.dragged() {
                let p = (pos + resp.drag_delta()).clamp(screen.min, max);
                self.settings.pomo_corner = Some([screen.max.x - p.x, screen.max.y - p.y]);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if resp.drag_stopped() {
                self.settings.save();
            }
            let c = pos2(r.center().x, r.min.y + 42.0);
            paint_tomato(ui.painter(), c, 32.0, self.pomo.fraction_left(&self.settings), self.pomo.phase, self.pomo.running, self.pomo.waiting(), now);
            if self.settings.pomo_show_time {
                let label = self.pomo.label(&self.settings);
                let g = ui.painter().layout_no_wrap(label, widgets::mono_font(13.0), Color32::WHITE);
                let pill = Rect::from_center_size(pos2(r.center().x, r.max.y - 12.0), g.size() + vec2(14.0, 6.0));
                ui.painter().rect_filled(pill, 9.0, with_alpha(Color32::BLACK, 170));
                ui.painter().galley(pill.center() - g.size() / 2.0, g, Color32::WHITE);
            }
            let resp = resp.on_hover_text(format!("{} · {}", self.pomo.phase_name(), tr!("Klick: Start/Pause · Ziehen: verschieben · Rechtsklick: Menü" | "Click: start/pause · drag: move · right click: menu")));
            if resp.clicked() {
                self.pomo.toggle(&self.settings);
            }
            resp.context_menu(|ui| {
                if ui.button(format!("{}  {}", if self.pomo.running { ic::STOP } else { ic::PLAY }, if self.pomo.running { tr!("Pausieren" | "Pause") } else { tr!("Starten" | "Start") })).clicked() {
                    self.pomo.toggle(&self.settings);
                    ui.close();
                }
                if ui.button(format!("{}  {}", ic::STEP, tr!("Überspringen" | "Skip"))).clicked() {
                    self.pomo.skip(&self.settings);
                    ui.close();
                }
                if ui.button(format!("{}  {}", ic::UNDO, tr!("Zurücksetzen" | "Reset"))).clicked() {
                    self.pomo.reset();
                    ui.close();
                }
                ui.separator();
                if ui.button(format!("{}  {}", ic::TIMES, tr!("Tomate ausblenden" | "Hide tomato"))).clicked() {
                    self.settings.pomo_float = false;
                    self.settings.save();
                    ui.close();
                }
            });
        });
        let _ = resp;
        let _ = pal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_valid_wav() {
        for k in [Chime::Short, Chime::Long] {
            let w = chime_wav(k);
            assert_eq!(&w[0..4], b"RIFF");
            assert_eq!(&w[8..12], b"WAVE");
            let data = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
            assert_eq!(w.len(), 44 + data);
        }
    }

    #[test]
    fn cycle() {
        let mut s = crate::app::Settings::default();
        s.pomo_rounds = 2;
        let mut p = Pomodoro::default();
        assert_eq!(p.label(&s), "25:00");
        p.toggle(&s);
        assert!(p.running);
        p.toggle(&s);
        assert!(!p.running && p.left(&s) > 1490.0);
        p.skip(&s);
        assert_eq!(p.phase, Phase::Short);
        assert_eq!(p.label(&s), "05:00");
    }
}
