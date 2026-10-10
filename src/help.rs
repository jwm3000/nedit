//! Help window: all keyboard shortcuts and features, in German and English.

use crate::app::App;
use crate::icons as ic;
use crate::theme::{mix, with_alpha, Palette};
use crate::widgets;
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Stroke, StrokeKind};

struct Section {
    icon: &'static str,
    title: &'static str,
    rows: Vec<(&'static str, &'static str)>,
}

fn sections() -> Vec<Section> {
    vec![
        Section {
            icon: ic::BOLT,
            title: tr!("Allgemein" | "General"),
            rows: vec![
                ("2× Shift / Ctrl+P", tr!("Schnellsuche: Dateien, Kapitel, Literatur, Befehle" | "Quick open: files, chapters, literature, commands")),
                ("Ctrl+S", tr!("Speichern & kompilieren – PDF springt zur Cursorstelle" | "Save & compile – PDF jumps to the cursor")),
                ("Ctrl+Enter", tr!("Kompilieren" | "Compile")),
                ("Alt+1 / 2 / 3", tr!("Masterarbeit / Präsentation / Bibliothek" | "Thesis / presentation / library")),
                ("F1", tr!("Diese Hilfe" | "This help")),
                ("Ctrl + / Ctrl −", tr!("Editor-Schrift größer / kleiner" | "Editor font larger / smaller")),
            ],
        },
        Section {
            icon: ic::SEARCH,
            title: tr!("Schnellsuche" | "Quick open"),
            rows: vec![
                ("↑ ↓  ↵  Esc", tr!("Auswählen, öffnen, schließen" | "Select, open, close")),
                ("# …", tr!("Nur Gliederung (Kapitel & Abschnitte)" | "Outline only (chapters & sections)")),
                ("@ …", tr!("Nur Literatur – Enter fügt \\citep{…} ein" | "Literature only – Enter inserts \\citep{…}")),
                ("> …", tr!("Nur Befehle (Kompilieren, Theme, Sprache …)" | "Commands only (compile, theme, language …)")),
                (":42", tr!("Zu Zeile 42 springen" | "Go to line 42")),
            ],
        },
        Section {
            icon: ic::PENCIL,
            title: tr!("Editor" | "Editor"),
            rows: vec![
                ("Ctrl+E", tr!("Code ↔ Visuell" | "Code ↔ visual")),
                ("Ctrl+L", tr!("PDF-Vorschau ein/aus" | "Toggle PDF preview")),
                ("Ctrl+Shift+D", tr!("Dokument-Modus (alle Kapitel am Stück)" | "Document mode (all chapters in one view)")),
                ("Ctrl+Tab", tr!("Nächste Datei – im Dokument-Modus nächstes Kapitel" | "Next file – next chapter in document mode")),
                ("Ctrl+Shift+Tab", tr!("Vorherige Datei / vorheriges Kapitel" | "Previous file / chapter")),
                ("F11 / Esc", tr!("Vollbild – nur der Text" | "Full screen – just the text")),
                ("Ctrl+F", tr!("Suchen & Ersetzen" | "Find & replace")),
                ("Ctrl+B / Ctrl+I", tr!("Fett / Kursiv" | "Bold / italic")),
                ("Ctrl+/", tr!("Zeilen (aus)kommentieren" | "Toggle comment")),
                ("Tab / Shift+Tab", tr!("Einrücken / Ausrücken" | "Indent / outdent")),
                ("Ctrl+Click", tr!("Stelle im PDF zeigen" | "Show position in the PDF")),
                (tr!("Doppelklick im PDF" | "Double-click in PDF"), tr!("Zur Quelltextstelle springen" | "Jump to the source")),
                ("\\cite{  \\cref{  \\begin{", tr!("Autovervollständigung" | "Autocompletion")),
                ("\\begin{…} ↵", tr!("Passendes \\end{…} wird automatisch ergänzt" | "Matching \\end{…} is added automatically")),
                ("{  [", tr!("Klammern werden paarweise gesetzt, Auswahl wird umschlossen" | "Brackets are paired, a selection gets wrapped")),
                ("\\item … ↵", tr!("Nächster Listenpunkt (leerer Punkt + ↵ beendet die Liste)" | "Next list item (empty item + ↵ ends the list)")),
            ],
        },
        Section {
            icon: ic::TERMINAL,
            title: tr!("Vim-Modus (⚙ → Eingabe: Vim)" | "Vim mode (⚙ → Input: Vim)"),
            rows: vec![
                ("i a I A o O  Esc", tr!("Einfügen / zurück in den Normal-Modus" | "Insert / back to normal mode")),
                ("h j k l  w b e  0 ^ $", tr!("Zeichen, Wörter, Zeilenanfang/-ende" | "Characters, words, line start/end")),
                ("gg G  {n}G  { }  %", tr!("Dateianfang/-ende, Zeile n, Absatz, Klammerpaar" | "File start/end, line n, paragraph, matching bracket")),
                ("f t F T  ;  ,", tr!("Zu Zeichen springen und wiederholen" | "Jump to character and repeat")),
                ("d c y  > <", tr!("Löschen, ändern, kopieren, einrücken – mit Bewegung" | "Delete, change, yank, indent – with a motion")),
                ("ci{  di$  daw  yip", tr!("Textobjekte: Klammerinhalt, Formel, Wort, Absatz" | "Text objects: brace content, formula, word, paragraph")),
                ("dd cc yy  x  p P  J  r  ~", tr!("Zeile löschen/ändern/kopieren, einfügen, verbinden …" | "Delete/change/yank line, put, join …")),
                ("u  Ctrl+R  .", tr!("Rückgängig, wiederholen, letzte Änderung wiederholen" | "Undo, redo, repeat last change")),
                ("v  V  Ctrl+V", tr!("Visuell: Zeichen, Zeilen, Block" | "Visual: characters, lines, block")),
                ("Ctrl+V … I / A", tr!("Text in allen Blockzeilen einfügen (z. B. % zum Auskommentieren)" | "Insert text on all block lines (e.g. % to comment out)")),
                ("/text  ?text", tr!("Vorwärts / rückwärts suchen" | "Search forward / backward")),
                ("n  N", tr!("Nächster Treffer in Suchrichtung / entgegen" | "Next match in search direction / opposite")),
                (":w  :q  :wq", tr!("Speichern & kompilieren, Tab schließen" | "Save & compile, close tab")),
                (":42  :%s/a/b/g  :noh", tr!("Zeile, Ersetzen, Hervorhebung aus" | "Go to line, substitute, clear highlight")),
            ],
        },
        Section {
            icon: ic::TV,
            title: tr!("Präsentation" | "Presentation"),
            rows: vec![
                ("F5", tr!("Präsentieren (Vollbild mit Timer)" | "Present (full screen with timer)")),
                (tr!("Visuell" | "Visual"), tr!("Folien direkt bearbeiten wie in PowerPoint – sauberes LaTeX entsteht automatisch" | "Edit slides directly like in PowerPoint – clean LaTeX is generated")),
                ("↵  Tab  ⇧Tab", tr!("Visuell: neuer Punkt, einrücken, ausrücken" | "Visual: new item, indent, outdent")),
                ("← →  Space", tr!("Folie zurück / weiter" | "Previous / next slide")),
                ("B  P  T  Esc", tr!("Schwarz, Pause, Leiste fixieren, beenden" | "Black, pause, pin bar, exit")),
            ],
        },
        Section {
            icon: ic::FOLDER,
            title: tr!("Dateien & Git" | "Files & Git"),
            rows: vec![
                ("F2  Del", tr!("Umbenennen / löschen (im Dateibaum)" | "Rename / delete (in the file tree)")),
                (tr!("Ziehen & Ablegen" | "Drag & drop"), tr!("Dateien verschieben oder von außen hinzufügen" | "Move files or add them from outside")),
                (tr!("Rechtsklick → Git" | "Right-click → Git"), tr!("Änderungen anzeigen oder verwerfen, Repository anlegen" | "Show or discard changes, create repository")),
                ("M  N", tr!("Geändert / neu seit dem letzten Commit" | "Modified / new since the last commit")),
            ],
        },
    ]
}

/// Key combination as small rounded keycaps.
fn keycaps(ui: &mut egui::Ui, keys: &str, pal: &Palette) {
    let font = widgets::mono_font(11.5);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, group) in keys.split("  ").enumerate() {
            if i > 0 {
                ui.add_space(4.0);
            }
            let g = ui.painter().layout_no_wrap(group.to_string(), font.clone(), pal.text);
            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 22.0), Sense::hover());
            ui.painter().rect_filled(r.translate(vec2(0.0, 1.5)), 5.0, with_alpha(Color32::BLACK, 40));
            ui.painter().rect_filled(r, 5.0, mix(pal.base, pal.surface, 0.4));
            ui.painter().rect_stroke(r, 5.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
            ui.painter().galley(pos2(r.min.x + 6.0, r.center().y - g.size().y / 2.0), g, pal.bright);
        }
    });
}

impl App {
    pub fn help_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.help_open {
            return;
        }
        let mut open = true;
        let screen = ctx.content_rect();
        let w = (screen.width() * 0.62).clamp(520.0, 820.0);
        egui::Window::new(egui::RichText::new(format!("{}  {}", ic::BOOKMARK, tr!("Hilfe & Tastenkürzel" | "Help & shortcuts"))).font(widgets::ui_font(13.0)).color(pal.text))
            .id(egui::Id::new("help-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .movable(true)
            .default_rect(Rect::from_center_size(screen.center(), vec2(w, (screen.height() * 0.78).min(760.0))))
            .frame(
                egui::Frame::new()
                    .fill(pal.surface)
                    .stroke(Stroke::new(1.0, pal.border))
                    .corner_radius(14)
                    .inner_margin(egui::Margin::same(18))
                    .shadow(egui::Shadow { offset: [0, 14], blur: 40, spread: 0, color: with_alpha(Color32::BLACK, 120) }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tr!("Alles auf einen Blick" | "Everything at a glance")).font(widgets::display_font(22.0)).color(pal.bright));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // language switch right inside the help
                        let mut en = crate::i18n::en();
                        let ch = ui.selectable_value(&mut en, true, "EN").changed() | ui.selectable_value(&mut en, false, "DE").changed();
                        if ch {
                            self.settings.lang_en = en;
                            crate::i18n::set_english(en);
                            self.settings.save();
                        }
                    });
                });
                ui.label(egui::RichText::new(tr!("Ctrl steht für Strg, Shift für Umschalt. Auf dem Mac gilt ⌘ statt Ctrl." | "On macOS use ⌘ instead of Ctrl.")).font(widgets::ui_font(11.5)).color(pal.dim));
                ui.add_space(10.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for sec in sections() {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                            ui.painter().rect_filled(r, 7.0, with_alpha(pal.accent, 34));
                            ui.painter().text(r.center(), Align2::CENTER_CENTER, sec.icon, widgets::ui_font(12.5), pal.accent);
                            ui.label(egui::RichText::new(sec.title).font(widgets::bold_font(14.5)).color(pal.bright));
                        });
                        ui.add_space(4.0);
                        egui::Frame::new().fill(pal.base).corner_radius(10).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let key_w = (ui.available_width() * 0.42).min(300.0);
                            for (i, (keys, desc)) in sec.rows.iter().enumerate() {
                                if i > 0 {
                                    let y = ui.cursor().min.y;
                                    let x = ui.max_rect();
                                    ui.painter().line_segment([pos2(x.min.x, y), pos2(x.max.x, y)], Stroke::new(1.0, with_alpha(pal.border, 70)));
                                }
                                ui.horizontal(|ui| {
                                    ui.set_min_height(30.0);
                                    ui.allocate_ui_with_layout(vec2(key_w, 26.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                        ui.set_min_width(key_w);
                                        keycaps(ui, keys, pal);
                                    });
                                    ui.label(egui::RichText::new(*desc).font(widgets::ui_font(13.0)).color(pal.text));
                                });
                            }
                        });
                        ui.add_space(14.0);
                    }
                });
            });
        if !open {
            self.help_open = false;
        }
    }
}
