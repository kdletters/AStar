mod block;
mod game;
// mod video_player;

use godot::classes::Engine;
use godot::prelude::*;
use godot_tokio::AsyncRuntime;

#[macro_export]
macro_rules! godot_print_err {
    ($fmt:literal $(, $args:expr)* $(,)?) => {
        godot::global::printerr(&[
            godot::builtin::Variant::from(
                format!($fmt $(, $args)*)
            )
        ])
    };
}

struct MyExtension;

#[gdextension]
unsafe impl ExtensionLibrary for MyExtension {
    fn on_level_init(level: InitLevel) {
        match level {
            InitLevel::Scene => {
                let mut engine = Engine::singleton();

                // This is where we register our async runtime singleton.
                godot_warn!("Success to add singleton -> {}", AsyncRuntime::SINGLETON);
                engine.register_singleton(AsyncRuntime::SINGLETON, &AsyncRuntime::new_alloc());
            }
            _ => (),
        }
    }

    fn on_level_deinit(level: InitLevel) {
        match level {
            InitLevel::Scene => {
                let mut engine = Engine::singleton();

                // Here is where we free our async runtime singleton from memory.
                if let Some(async_singleton) = engine.get_singleton(AsyncRuntime::SINGLETON) {
                    engine.unregister_singleton(AsyncRuntime::SINGLETON);
                    async_singleton.free();
                } else {
                    godot_warn!(
                        "Failed to find & free singleton -> {}",
                        AsyncRuntime::SINGLETON
                    );
                }
            }
            _ => (),
        }
    }
}
