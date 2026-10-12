//! Focus sounds: noise colours, rain, sea and a fireplace – synthesized live (no samples, no
//! loops) and played through the default audio output with cpal.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Sound {
    White,
    Pink,
    #[default]
    Brown,
    Rain,
    Ocean,
    Fire,
}

impl Sound {
    pub const ALL: [Sound; 6] = [Sound::Brown, Sound::Pink, Sound::White, Sound::Rain, Sound::Ocean, Sound::Fire];

    pub fn name(self) -> &'static str {
        match self {
            Sound::White => tr!("Weißes Rauschen" | "White noise"),
            Sound::Pink => tr!("Rosa Rauschen" | "Pink noise"),
            Sound::Brown => tr!("Braunes Rauschen" | "Brown noise"),
            Sound::Rain => tr!("Regen" | "Rain"),
            Sound::Ocean => tr!("Meer" | "Sea"),
            Sound::Fire => tr!("Kaminfeuer" | "Fireplace"),
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Sound::White => tr!("Hell, deckt alles ab" | "Bright, masks everything"),
            Sound::Pink => tr!("Ausgewogen, wie Wind" | "Balanced, like wind"),
            Sound::Brown => tr!("Tief und warm" | "Deep and warm"),
            Sound::Rain => tr!("Regen am Fenster" | "Rain on the window"),
            Sound::Ocean => tr!("Wellen, die heranrollen" | "Rolling waves"),
            Sound::Fire => tr!("Knistern im Kamin" | "Crackling fire"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Sound::White | Sound::Pink | Sound::Brown => "\u{f147d}", // md-waveform
            Sound::Rain => "\u{f0596}",                                // md-weather-pouring
            Sound::Ocean => "\u{f078d}",                               // md-waves
            Sound::Fire => "\u{f0238}",                                // md-fire
        }
    }

    fn id(self) -> u8 {
        Sound::ALL.iter().position(|s| *s == self).unwrap_or(0) as u8
    }

    fn from_id(i: u8) -> Sound {
        Sound::ALL.get(i as usize).copied().unwrap_or_default()
    }
}

/// Shared between the UI and the audio callback.
struct Shared {
    sound: AtomicU8,
    volume: AtomicU32, // f32 bits
    playing: AtomicBool,
}

#[derive(Default)]
pub struct Noise {
    shared: Option<Arc<Shared>>,
    stream: Option<cpal::Stream>,
    /// when playback was paused (the stream is closed a little later)
    paused_at: Option<Instant>,
    pub error: Option<String>,
    /// last pomodoro focus state seen (for "play during focus rounds")
    pub pomo_prev: Option<bool>,
}

impl Noise {
    pub fn playing(&self) -> bool {
        self.shared.as_ref().is_some_and(|s| s.playing.load(Ordering::Relaxed))
    }

    /// Start or stop with the given sound and volume (0..1).
    pub fn set_playing(&mut self, on: bool, sound: Sound, volume: f32) {
        if on && self.stream.is_none() {
            if let Err(e) = self.open() {
                self.error = Some(e);
                return;
            }
        }
        self.error = None;
        if let Some(s) = &self.shared {
            s.sound.store(sound.id(), Ordering::Relaxed);
            s.volume.store(volume.to_bits(), Ordering::Relaxed);
            s.playing.store(on, Ordering::Relaxed);
        }
        self.paused_at = (!on).then(Instant::now);
    }

    pub fn update(&self, sound: Sound, volume: f32) {
        if let Some(s) = &self.shared {
            s.sound.store(sound.id(), Ordering::Relaxed);
            s.volume.store(volume.to_bits(), Ordering::Relaxed);
        }
    }

    /// Close the audio stream some time after pausing (frees the device).
    pub fn tick(&mut self) {
        if self.paused_at.is_some_and(|t| t.elapsed().as_secs_f32() > 3.0) {
            self.stream = None;
            self.shared = None;
            self.paused_at = None;
        }
    }

    fn open(&mut self) -> Result<(), String> {
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or_else(|| tr!("Keine Audioausgabe gefunden" | "No audio output found").to_string())?;
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared { sound: AtomicU8::new(0), volume: AtomicU32::new(0.5f32.to_bits()), playing: AtomicBool::new(false) });
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&dev, cfg.config(), shared.clone()),
            cpal::SampleFormat::I16 => build::<i16>(&dev, cfg.config(), shared.clone()),
            cpal::SampleFormat::U16 => build::<u16>(&dev, cfg.config(), shared.clone()),
            cpal::SampleFormat::I32 => build::<i32>(&dev, cfg.config(), shared.clone()),
            f => Err(format!("{f:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        self.shared = Some(shared);
        Ok(())
    }
}

fn build<T>(dev: &cpal::Device, cfg: cpal::StreamConfig, shared: Arc<Shared>) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = cfg.channels as usize;
    let mut synth = Synth::new(cfg.sample_rate as f32);
    dev.build_output_stream::<T, _, _>(
        cfg,
        move |data: &mut [T], _| {
            let want = Sound::from_id(shared.sound.load(Ordering::Relaxed));
            let vol = f32::from_bits(shared.volume.load(Ordering::Relaxed));
            let playing = shared.playing.load(Ordering::Relaxed);
            for frame in data.chunks_mut(channels) {
                let (l, r) = synth.next(want, if playing { vol } else { 0.0 });
                for (c, s) in frame.iter_mut().enumerate() {
                    *s = T::from_sample(if c % 2 == 0 { l } else { r });
                }
            }
        },
        |e| eprintln!("audio: {e}"),
        None,
    )
    .map_err(|e| e.to_string())
}

// ───────────────────────────── synthesis ─────────────────────────────

/// Noise state of one channel.
#[derive(Default, Clone)]
struct Chan {
    rng: u64,
    pink: [f32; 7],
    brown: f32,
    lp: [f32; 4],
    drop_env: f32,
    drop_lp: f32,
    crack_env: f32,
}

impl Chan {
    fn white(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let v = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (v >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }

    fn rand01(&mut self) -> f32 {
        (self.white() + 1.0) * 0.5
    }

    fn pink(&mut self, w: f32) -> f32 {
        // Paul Kellet's filter
        let b = &mut self.pink;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.1538520;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let p = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115926;
        p * 0.11
    }

    fn brown(&mut self, w: f32) -> f32 {
        self.brown = (self.brown + 0.02 * w) / 1.02;
        self.brown * 3.5
    }
}

/// One-pole low-pass coefficient for a cutoff frequency.
fn coef(fc: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * fc / sr).exp()
}

struct Synth {
    sr: f32,
    ch: [Chan; 2],
    cur: Sound,
    gain: f32,
    // sea: phase of the current wave (0..1), its length and height
    wave: f32,
    wave_len: f32,
    wave_peak: f32,
    // rain / fire: slow intensity drift
    drift: f32,
    drift_target: f32,
}

impl Synth {
    fn new(sr: f32) -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
        let mut ch = [Chan::default(), Chan::default()];
        ch[0].rng = seed;
        ch[1].rng = seed.rotate_left(29) ^ 0x9e37_79b9_7f4a_7c15;
        Synth { sr, ch, cur: Sound::Brown, gain: 0.0, wave: 0.0, wave_len: 9.0, wave_peak: 0.8, drift: 0.7, drift_target: 0.7 }
    }

    /// Next stereo frame. Switching sounds fades out and in.
    fn next(&mut self, want: Sound, vol: f32) -> (f32, f32) {
        let target = if want == self.cur { vol } else { 0.0 };
        let k = 1.0 / (0.12 * self.sr);
        self.gain += (target - self.gain) * k * 8.0;
        if want != self.cur && self.gain < 0.002 {
            self.cur = want;
        }
        if self.gain < 1e-5 && target == 0.0 {
            return (0.0, 0.0);
        }
        // shared slow modulations
        let sr = self.sr;
        if self.ch[0].rand01() < 1.0 / sr {
            self.drift_target = 0.55 + 0.45 * self.ch[0].rand01();
        }
        self.drift += (self.drift_target - self.drift) * (1.0 / (3.0 * sr));
        let mut env = 0.0;
        if self.cur == Sound::Ocean {
            self.wave += 1.0 / (self.wave_len * sr);
            if self.wave >= 1.0 {
                self.wave = 0.0;
                self.wave_len = 7.0 + 6.0 * self.ch[0].rand01();
                self.wave_peak = 0.55 + 0.45 * self.ch[0].rand01();
            }
            // the wave builds up slowly, breaks, and washes out
            let p = self.wave;
            let shape = if p < 0.45 { (p / 0.45).powf(1.6) } else { (-(p - 0.45) * 5.0).exp() };
            env = shape * self.wave_peak;
        }
        let (cur, drift) = (self.cur, self.drift);
        let mut out = [0.0f32; 2];
        for (i, c) in self.ch.iter_mut().enumerate() {
            let w = c.white();
            out[i] = match cur {
                Sound::White => w * 0.35,
                Sound::Pink => c.pink(w) * 0.9,
                Sound::Brown => c.brown(w) * 0.9,
                Sound::Rain => {
                    // a soft hiss of distant rain …
                    let p = c.pink(w);
                    c.lp[0] += (p - c.lp[0]) * coef(500.0, sr);
                    let hiss = (p - c.lp[0]) * 1.1 * drift;
                    // … and single drops: short bright ticks of noise
                    if c.rand01() < 55.0 * drift / sr {
                        c.drop_env = 0.15 + 0.85 * c.rand01().powi(3);
                    }
                    c.drop_env *= 1.0 - 1.0 / (0.006 * sr);
                    c.drop_lp += (w - c.drop_lp) * coef(2500.0, sr);
                    let drop = (w - c.drop_lp) * c.drop_env * 0.9;
                    // low rumble of the rain on the roof
                    let rumble = c.brown(w) * 0.25;
                    (hiss + drop + rumble) * 1.3
                }
                Sound::Ocean => {
                    // brown base, the wave opens a low-pass and adds foam
                    let b = c.brown(w) * 0.5;
                    let fc = 250.0 + 2200.0 * env * env;
                    c.lp[0] += (w - c.lp[0]) * coef(fc, sr);
                    c.lp[1] += (c.lp[0] - c.lp[1]) * coef(fc, sr);
                    let surf = c.lp[1] * (0.25 + 2.2 * env);
                    (b * (0.5 + 0.6 * env) + surf) * 1.45
                }
                Sound::Fire => {
                    // warm low roar …
                    c.lp[0] += (w - c.lp[0]) * coef(180.0, sr);
                    let roar = c.lp[0] * 3.0 * (0.7 + 0.3 * drift);
                    // … crackles and pops, sometimes in little clusters
                    let rate = if c.crack_env > 0.05 { 60.0 } else { 6.0 };
                    if c.rand01() < rate / sr {
                        c.crack_env = c.crack_env.max(0.2 + 0.8 * c.rand01().powi(2));
                    }
                    c.crack_env *= 1.0 - 1.0 / (0.003 * sr);
                    c.lp[1] += (w - c.lp[1]) * coef(1200.0, sr);
                    let crack = (w - c.lp[1]) * c.crack_env * 1.4;
                    roar + crack
                }
            };
        }
        let g = self.gain * 0.9;
        // soft clip
        (fast_tanh(out[0] * g), fast_tanh(out[1] * g))
    }
}

fn fast_tanh(x: f32) -> f32 {
    let x2 = x * x;
    (x * (27.0 + x2) / (27.0 + 9.0 * x2)).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_audible_and_bounded() {
        for s in Sound::ALL {
            let mut syn = Synth::new(44_100.0);
            syn.cur = s;
            let mut sum = 0.0f64;
            let n = 44_100 * 3;
            for _ in 0..n {
                let (l, r) = syn.next(s, 0.8);
                assert!(l.abs() <= 1.0 && r.abs() <= 1.0, "{s:?}");
                sum += (l * l) as f64;
            }
            let rms = (sum / n as f64).sqrt();
            assert!(rms > 0.01 && rms < 0.6, "{s:?} rms {rms}");
        }
    }

    /// Opens the real audio output (silently, volume 0).
    #[test]
    #[ignore]
    fn opens_audio_output() {
        let mut n = Noise::default();
        n.set_playing(true, Sound::Rain, 0.0);
        assert!(n.error.is_none(), "{:?}", n.error);
        assert!(n.playing());
        std::thread::sleep(std::time::Duration::from_millis(400));
        n.set_playing(false, Sound::Rain, 0.0);
    }

    #[test]
    fn switching_fades() {
        let mut syn = Synth::new(44_100.0);
        for _ in 0..44_100 {
            syn.next(Sound::Brown, 0.8);
        }
        let mut switched = false;
        for _ in 0..44_100 {
            syn.next(Sound::Rain, 0.8);
            switched |= syn.cur == Sound::Rain;
        }
        assert!(switched && syn.gain > 0.5);
    }
}

// ───────────────────────────── App integration ─────────────────────────────

use crate::app::App;
use crate::theme::{with_alpha, Palette};
use crate::widgets;
use egui::{pos2, vec2, Align2, Rect, Sense, Stroke};

impl App {
    pub fn noise_set(&mut self, on: bool, now: f64) {
        self.noise.set_playing(on, self.settings.noise_sound, self.settings.noise_volume);
        if let Some(e) = self.noise.error.take() {
            let r = self.pal.red;
            self.toast(crate::icons::WARN, trf!("Kein Ton: {e}" | "No sound: {e}"), r, now);
        }
    }

    pub fn noise_toggle(&mut self, now: f64) {
        let on = !self.noise.playing();
        self.noise_set(on, now);
    }

    /// Every frame: settings → audio thread, pomodoro coupling, closing an idle stream.
    pub fn noise_tick(&mut self, now: f64) {
        self.noise.tick();
        if !self.settings.noise_enabled {
            if self.noise.playing() {
                self.noise_set(false, now);
            }
            return;
        }
        self.noise.update(self.settings.noise_sound, self.settings.noise_volume);
        if self.settings.noise_with_pomo {
            let focus = self.pomo.running && self.pomo.phase == crate::pomodoro::Phase::Work;
            if self.noise.pomo_prev != Some(focus) {
                self.noise.pomo_prev = Some(focus);
                if focus != self.noise.playing() {
                    self.noise_set(focus, now);
                }
            }
        } else {
            self.noise.pomo_prev = None;
        }
    }

    /// Play/pause pill in the top bar (right of the tabs); right click picks the sound.
    pub fn noise_button(&mut self, ui: &mut egui::Ui, pal: &Palette, now: f64) {
        if !self.settings.noise_enabled {
            return;
        }
        let playing = self.noise.playing();
        let (r, resp) = ui.allocate_exact_size(vec2(58.0, 30.0), Sense::click());
        let hover = resp.hovered();
        let fill = if playing { with_alpha(pal.accent, if hover { 60 } else { 40 }) } else if hover { pal.surface } else { with_alpha(pal.surface, 160) };
        ui.painter().rect(r, 15.0, fill, Stroke::new(1.0, with_alpha(if playing { pal.accent } else { pal.border }, 140)), egui::StrokeKind::Inside);
        let col = if playing { pal.accent } else { pal.subtext };
        let icon_c = pos2(r.min.x + 17.0, r.center().y);
        ui.painter().text(icon_c, Align2::CENTER_CENTER, if playing { crate::icons::PAUSE } else { crate::icons::PLAY }, widgets::ui_font(11.0), col);
        // little level meter: dances while playing, rests otherwise
        for i in 0..4 {
            let x = r.min.x + 32.0 + i as f32 * 5.0;
            let h = if playing { 4.0 + 9.0 * (0.5 + 0.5 * ((now * (3.1 + i as f64 * 1.7)) + i as f64 * 1.3).sin() as f32) } else { 3.0 };
            ui.painter().rect_filled(Rect::from_center_size(pos2(x, r.center().y), vec2(2.6, h)), 1.3, col);
        }
        if playing {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
        }
        let resp = resp.on_hover_text(format!(
            "{} · {}",
            self.settings.noise_sound.name(),
            tr!("Klick: abspielen/pausieren · Rechtsklick: Geräusch wählen" | "Click: play/pause · right click: choose a sound")
        ));
        if resp.clicked() {
            self.noise_toggle(now);
        }
        resp.context_menu(|ui| {
            ui.set_min_width(220.0);
            for s in Sound::ALL {
                let sel = self.settings.noise_sound == s;
                if ui.selectable_label(sel, format!("{}  {}", s.icon(), s.name())).clicked() {
                    self.settings.noise_sound = s;
                    self.settings.save();
                    if !self.noise.playing() {
                        self.noise_set(true, now);
                    }
                }
            }
            ui.separator();
            let mut v = self.settings.noise_volume * 100.0;
            ui.horizontal(|ui| {
                ui.label(crate::icons::VOLUME);
                if ui.add(egui::Slider::new(&mut v, 0.0..=100.0).show_value(false)).changed() {
                    self.settings.noise_volume = v / 100.0;
                }
                if ui.input(|i| i.pointer.any_released()) {
                    self.settings.save();
                }
            });
        });
    }

    /// "Focus sounds" section of the focus settings page.
    pub fn noise_settings(&mut self, ui: &mut egui::Ui, pal: &Palette, now: f64) {
        // sound cards, 3 per row
        let gap = 10.0;
        let w = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
        let playing = self.noise.playing();
        for row in Sound::ALL.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &s in row {
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 74.0), Sense::click());
                    let sel = self.settings.noise_sound == s;
                    let live = sel && playing;
                    let hover = resp.hovered();
                    let fill = if sel { with_alpha(pal.accent, 30) } else if hover { pal.surface } else { crate::theme::mix(pal.surface, pal.base, 0.4) };
                    ui.painter().rect(r, 12.0, fill, Stroke::new(if sel { 1.5 } else { 1.0 }, if sel { pal.accent } else { with_alpha(pal.border, 120) }), egui::StrokeKind::Inside);
                    let col = if sel { pal.accent } else { pal.subtext };
                    ui.painter().text(pos2(r.min.x + 16.0, r.min.y + 22.0), Align2::LEFT_CENTER, s.icon(), widgets::ui_font(18.0), col);
                    ui.painter().text(pos2(r.min.x + 42.0, r.min.y + 22.0), Align2::LEFT_CENTER, s.name(), widgets::bold_font(13.0), pal.bright);
                    ui.painter().text(pos2(r.min.x + 16.0, r.min.y + 50.0), Align2::LEFT_CENTER, s.hint(), widgets::ui_font(11.5), pal.dim);
                    // play state in the corner
                    let c = pos2(r.max.x - 18.0, r.min.y + 22.0);
                    if live {
                        for i in 0..3 {
                            let h = 3.0 + 8.0 * (0.5 + 0.5 * ((now * (3.3 + i as f64 * 1.9)) + i as f64).sin() as f32);
                            ui.painter().rect_filled(Rect::from_center_size(pos2(c.x - 5.0 + i as f32 * 5.0, c.y), vec2(2.6, h)), 1.3, pal.accent);
                        }
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
                    } else if hover {
                        ui.painter().text(c, Align2::CENTER_CENTER, crate::icons::PLAY, widgets::ui_font(10.0), col);
                    }
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        if live {
                            self.noise_set(false, now);
                        } else {
                            self.settings.noise_sound = s;
                            self.settings.save();
                            self.noise_set(true, now);
                        }
                    }
                }
            });
            ui.add_space(gap);
        }
    }
}
