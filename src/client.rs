use std::collections::HashMap;
use std::error::Error;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::{io, thread};

use ggez::conf::{WindowMode, WindowSetup};
use ggez::event::{self, EventHandler};
use ggez::glam::Vec2;
use ggez::graphics::{self, Color, DrawParam, Mesh, PxScale, Rect, Text, TextFragment};
use ggez::winit::keyboard::{KeyCode, PhysicalKey};
use ggez::{Context, ContextBuilder, GameResult};

#[derive(Debug)]
struct PlayerPayload {
    id: u64,
    name: String,
    position: Vec2,
}

#[derive(Debug)]
enum PlayerPayloadParseError {
    Format { len: usize },
    Values { error: String, index: usize },
}

impl std::fmt::Display for PlayerPayloadParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Format { len } => write!(
                f,
                "Expected 4 spaced args 'id name X Y', received {len} tokens"
            ),
            Self::Values { error, index } => {
                write!(f, "Could not parse token {index}: {error}")
            }
        }
    }
}

impl TryFrom<&str> for PlayerPayload {
    type Error = PlayerPayloadParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let splits: Vec<&str> = value.split_whitespace().collect();
        if splits.len() != 4 {
            return Err(PlayerPayloadParseError::Format { len: splits.len() });
        }

        Ok(Self {
            id: splits[0]
                .parse::<u64>()
                .map_err(|e| PlayerPayloadParseError::Values {
                    error: e.to_string(),
                    index: 0,
                })?,
            name: splits[1].to_string(),
            position: Vec2 {
                x: splits[2]
                    .parse::<f32>()
                    .map_err(|e| PlayerPayloadParseError::Values {
                        error: e.to_string(),
                        index: 2,
                    })?,
                y: splits[3]
                    .parse::<f32>()
                    .map_err(|e| PlayerPayloadParseError::Values {
                        error: e.to_string(),
                        index: 3,
                    })?,
            },
        })
    }
}

#[derive(Debug)]
struct WorldPayload {
    tick: u64,
    players: Vec<PlayerPayload>,
}

#[derive(Debug)]
enum WorldPayloadParseError {
    MissingTick,
    InvalidTick(std::num::ParseIntError),
    InvalidPlayer {
        line: usize,
        error: PlayerPayloadParseError,
    },
}

impl std::fmt::Display for WorldPayloadParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTick => write!(f, "Expected a tick on the first line"),
            Self::InvalidTick(error) => write!(f, "Could not parse tick: {error}"),
            Self::InvalidPlayer { line, error } => {
                write!(f, "Could not parse player on line {line}: {error}")
            }
        }
    }
}

impl TryFrom<&str> for WorldPayload {
    type Error = WorldPayloadParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let mut lines = value.trim_end_matches(['\r', '\n']).lines();
        let tick = lines
            .next()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .ok_or(WorldPayloadParseError::MissingTick)?
            .parse::<u64>()
            .map_err(WorldPayloadParseError::InvalidTick)?;

        let players = lines
            .enumerate()
            .map(|(index, line)| {
                PlayerPayload::try_from(line).map_err(|error| {
                    WorldPayloadParseError::InvalidPlayer {
                        line: index + 2,
                        error,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { tick, players })
    }
}

struct PlayerState {
    id: u64,
    position: Vec2,
    name: String,
    // speed: f32,
    size: Vec2,
    color: Color,
}

struct MainState {
    socket: UdpSocket,
    receiver: Receiver<WorldPayload>,
    my_name: String,
    my_id: Option<u64>,
    players: HashMap<u64, PlayerState>,
    clear_color: Color,
}

impl MainState {
    fn new(
        _ctx: &mut Context,
        socket: UdpSocket,
        receiver: Receiver<WorldPayload>,
        my_name: String,
    ) -> GameResult<MainState> {
        Ok(MainState {
            socket,
            receiver,
            my_name,
            my_id: None,
            players: HashMap::new(),
            clear_color: Color::from_rgb(20, 30, 50),
        })
    }
}

impl EventHandler for MainState {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
        // let window_size = ctx.gfx.drawable_size();
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

        _ = self
            .socket
            .send(format!("{} {} {}", self.my_name, direction.x, direction.y).as_bytes());

        for payload in self.receiver.try_iter() {
            for player in payload.players {
                let player_state = self.players.entry(player.id).or_insert(PlayerState {
                    id: player.id,
                    position: player.position,
                    name: player.name.clone(),
                    size: Vec2::new(50.0, 50.0),
                    color: Color::CYAN,
                });

                player_state.name = player.name;
                player_state.position = player.position;
            }
        }

        // self.player_state.position +=
        //     direction * self.player_state.speed * ctx.time.delta().as_secs_f32();
        // self.player_state.position = Vec2::new(
        //     self.player_state.position.x.rem_euclid(window_size.0),
        //     self.player_state.position.y.rem_euclid(window_size.1),
        // );

        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let mut canvas = graphics::Canvas::from_frame(ctx, self.clear_color);

        for player in self.players.values() {
            let rect = Rect::new(
                player.position.x,
                player.position.y,
                player.size.x,
                player.size.y,
            );
            let mesh = Mesh::new_rectangle(ctx, graphics::DrawMode::fill(), rect, player.color)?;
            canvas.draw(&mesh, DrawParam::default());

            let text = Text::new(TextFragment {
                text: player.name.clone(),
                color: Some(Color::WHITE),
                scale: Some(PxScale::from(24.0)),
                ..Default::default()
            });
            canvas.draw(
                &text,
                DrawParam::default().dest(player.position + Vec2::NEG_Y * 25.0),
            );
        }

        canvas.finish(ctx)?;
        Ok(())
    }
}

type ClientResult = Result<(), Box<dyn Error + Send + Sync>>;

pub fn main() -> ClientResult {
    // Params
    let mut args = std::env::args();
    let address = args.nth(1).unwrap_or_else(|| "127.0.0.1:4000".to_string());
    let name = args.next().unwrap_or_else(|| "bob".to_string());

    // Socket
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let local_addr = socket.local_addr()?;
    println!("INFO: Socket bound to {local_addr}");

    socket.connect(&address)?;
    println!("INFO: socket connected to {address}");

    // Channel
    let (sender, receiver) = sync_channel(2048);

    // Receive thread
    let running = Arc::new(AtomicBool::new(true));
    let rcv_socket = socket.try_clone()?;
    let rcv_running = running.clone();
    let rcv_thread = thread::spawn(move || -> io::Result<()> {
        let mut buffer = [0u8; 1024];
        while rcv_running.load(Ordering::Relaxed) {
            let len = rcv_socket.recv(&mut buffer)?;
            let Ok(message) = str::from_utf8(&buffer[..len]) else {
                continue;
            };
            let Ok(payload) = WorldPayload::try_from(message) else {
                continue;
            };

            if sender.send(payload).is_err() {
                break;
            }
        }
        Ok(())
    });

    // Geez
    let (mut ctx, event_loop) = ContextBuilder::new("les_cubes", "pjdevs")
        .window_setup(WindowSetup::default().title("Les Cubes"))
        .window_mode(WindowMode::default().dimensions(800.0, 600.0))
        .build()?;

    let state = MainState::new(&mut ctx, socket, receiver, name)?;
    event::run(ctx, event_loop, state)?;

    running.store(false, Ordering::Relaxed);

    rcv_thread
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))?;

    Ok(())
}
