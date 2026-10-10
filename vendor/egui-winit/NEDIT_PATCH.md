# egui-winit 0.36.2 – nEdit patch

Unmodified copy of egui-winit 0.36.2 (MIT OR Apache-2.0, https://github.com/emilk/egui)
with one change in `src/lib.rs` (search for "nEdit patch"):

Upstream drops Ctrl+V completely when the clipboard is empty or cannot be read. nEdit passes
it on as a normal key event in that case, so Vim's visual-block mode (Ctrl+V) always works.
