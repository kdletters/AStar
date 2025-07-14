use godot::classes::*;
use godot::prelude::*;

#[derive(GodotClass)]
#[class(init, base = Panel)]
pub struct Block {
    base: Base<Panel>,

    #[init(node = "FLabel")]
    f_label: OnReady<Gd<Label>>,
    #[init(node = "GLabel")]
    g_label: OnReady<Gd<Label>>,
    #[init(node = "HLabel")]
    h_label: OnReady<Gd<Label>>,
    #[init(node = "PosLabel")]
    pos_label: OnReady<Gd<Label>>,
    #[init(node = "Button")]
    button: OnReady<Gd<Button>>,

    pos: (i32, i32),
    is_wall: bool,
    original_color: Color,
}

#[godot_api]
impl IPanel for Block {
    fn ready(&mut self) {
        self.original_color = Color::WHITE;
        self.is_wall = false;
        self.set_color(self.original_color);
        self.reset_labels();

        // Connect the button's pressed signal to our method
        self.button.signals().pressed().connect_other(self, Self::on_button_pressed);
    }
}

impl Block {
    pub fn set_f(&mut self, f: i32) {
        self.f_label.set_text(&f.to_string());
    }

    pub fn set_g(&mut self, g: i32) {
        self.g_label.set_text(&g.to_string());
    }

    pub fn set_h(&mut self, h: i32) {
        self.h_label.set_text(&h.to_string());
    }

    pub fn reset_labels(&mut self) {
        self.f_label.set_text("");
        self.g_label.set_text("");
        self.h_label.set_text("");
    }

    pub fn set_pos(&mut self, x: i32, y: i32) {
        self.pos = (x, y);
        self.pos_label.set_text(&format!("({},{})", x, y));
    }

    pub fn set_color(&mut self, color: Color) {
        self.base_mut().set_self_modulate(color);
    }

    pub fn set_as_wall(&mut self) {
        self.is_wall = true;
        self.set_color(crate::game::Game::WALL_BLOCK_COLOR);
    }

    pub fn is_wall(&self) -> bool {
        self.is_wall
    }

    pub fn reset_color(&mut self) {
        if !self.is_wall {
            self.set_color(self.original_color);
        }
        self.reset_labels();
    }
}

#[godot_api]
impl Block {
    fn on_button_pressed(&mut self) {
        let x = self.pos.0;
        let y = self.pos.1;
        let args = &[x.to_variant(), y.to_variant()];
        self.base_mut().emit_signal("clicked", args);
    }

    #[signal]
    pub fn clicked(x: i32, y: i32);
}
