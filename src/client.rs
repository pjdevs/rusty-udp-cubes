use ggez::conf::{WindowMode, WindowSetup};
use ggez::event::{self, EventHandler};
use ggez::glam::Vec2;
use ggez::graphics::{self, Color, DrawParam, Mesh, Rect};
use ggez::winit::keyboard::{KeyCode, PhysicalKey};
use ggez::{Context, ContextBuilder, GameResult};

struct PlayerState {
    position: Vec2,
    speed: f32,
    size: Vec2,
    color: Color,
}

struct MainState {
    player_state: PlayerState,
    clear_color: Color,
}

impl MainState {
    fn new(_ctx: &mut Context) -> GameResult<MainState> {
        Ok(MainState {
            player_state: PlayerState {
                position: Vec2::new(100.0, 250.0),
                speed: 250.0,
                size: Vec2::new(50.0, 50.0),
                color: Color::YELLOW,
            },
            clear_color: Color::from_rgb(20, 30, 50),
        })
    }
}

impl EventHandler for MainState {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
        let window_size = ctx.gfx.drawable_size();
        let mut direction = Vec2::ZERO;

        if ctx
            .keyboard
            .is_physical_key_pressed(&PhysicalKey::Code(KeyCode::ArrowLeft))
        {
            direction.x -= 1.0;
        }
        if ctx
            .keyboard
            .is_physical_key_pressed(&PhysicalKey::Code(KeyCode::ArrowRight))
        {
            direction.x += 1.0;
        }
        if ctx
            .keyboard
            .is_physical_key_pressed(&PhysicalKey::Code(KeyCode::ArrowUp))
        {
            direction.y -= 1.0;
        }
        if ctx
            .keyboard
            .is_physical_key_pressed(&PhysicalKey::Code(KeyCode::ArrowDown))
        {
            direction.y += 1.0;
        }

        direction = direction.normalize_or_zero();

        self.player_state.position +=
            direction * self.player_state.speed * ctx.time.delta().as_secs_f32();
        self.player_state.position = Vec2::new(
            self.player_state.position.x.rem_euclid(window_size.0),
            self.player_state.position.y.rem_euclid(window_size.1),
        );

        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let mut canvas = graphics::Canvas::from_frame(ctx, self.clear_color);

        let rect = Rect::new(
            self.player_state.position.x,
            self.player_state.position.y,
            self.player_state.size.x,
            self.player_state.size.y,
        );
        let mesh = Mesh::new_rectangle(
            ctx,
            graphics::DrawMode::fill(),
            rect,
            self.player_state.color,
        )?;
        canvas.draw(&mesh, DrawParam::default());

        canvas.finish(ctx)?;
        Ok(())
    }
}

pub fn main() -> GameResult {
    let (mut ctx, event_loop) = ContextBuilder::new("les_cubes", "pjdevs")
        .window_setup(WindowSetup::default().title("Les Cubes"))
        .window_mode(WindowMode::default().dimensions(800.0, 600.0))
        .build()?;

    let state = MainState::new(&mut ctx)?;
    event::run(ctx, event_loop, state)
}
