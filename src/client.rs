mod shared;

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{io, thread};

use ggez::conf::{WindowMode, WindowSetup};
use ggez::event::{self, EventHandler};
use ggez::glam::Vec2;
use ggez::graphics::{self, Color, DrawParam, Mesh, PxScale, Rect, Text, TextFragment};
use ggez::winit::keyboard::{KeyCode, PhysicalKey};
use ggez::{Context, ContextBuilder, GameResult};

use crate::shared::WORLD_SIZE;

#[derive(Debug, Default, Clone)]
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

#[derive(Debug, Default, Clone)]
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
    position: Vec2,
    name: String,
    mesh: Mesh,
    name_text: Text,
}

struct MainState {
    socket: Arc<UdpSocket>,
    cell: SharedCell<KnownServerState>,
    my_name: String,
    my_id: Option<u64>,
    players: HashMap<u64, PlayerState>,
    clear_color: Color,
}

impl MainState {
    fn new(
        _ctx: &mut Context,
        socket: Arc<UdpSocket>,
        cell: SharedCell<KnownServerState>,
        my_name: String,
    ) -> GameResult<MainState> {
        Ok(MainState {
            socket,
            cell,
            my_name,
            my_id: None,
            players: HashMap::new(),
            clear_color: Color::from_rgb(20, 30, 50),
        })
    }
}

impl EventHandler for MainState {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
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

        let state = if let Ok(s) = self.cell.lock() {
            s.clone()
        } else {
            return Err(ggez::GameError::LockError);
        };

        let mut known_players = HashSet::new();

        for player in state.last_state.players {
            let player_state = self.players.entry(player.id).or_insert_with(|| {
                let position = player.position;
                let size = Vec2::new(50.0, 50.0);
                let color = Color::CYAN;
                let mesh = Mesh::new_rectangle(
                    ctx,
                    graphics::DrawMode::fill(),
                    Rect::new(0.0, 0.0, size.x, size.y),
                    color,
                )
                .expect("Should be able to create a mesh here");
                let name_text = Text::new(TextFragment {
                    text: player.name.clone(),
                    color: Some(Color::WHITE),
                    scale: Some(PxScale::from(24.0)),
                    ..Default::default()
                });
                PlayerState {
                    position,
                    name: player.name.clone(),
                    mesh,
                    name_text,
                }
            });

            player_state.position = player.position;

            if player.name != player_state.name {
                player_state.name = player.name;
                player_state.name_text = Text::new(TextFragment {
                    text: player_state.name.clone(),
                    color: Some(Color::WHITE),
                    scale: Some(PxScale::from(24.0)),
                    ..Default::default()
                });
            }

            known_players.insert(player.id);
        }

        self.players
            .retain(|player_id, _| known_players.contains(player_id));

        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let mut canvas = graphics::Canvas::from_frame(ctx, self.clear_color);

        for player in self.players.values() {
            canvas.draw(&player.mesh, DrawParam::default().dest(player.position));
            canvas.draw(
                &player.name_text,
                DrawParam::default().dest(player.position + Vec2::NEG_Y * 25.0),
            );
        }

        canvas.finish(ctx)?;
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
struct KnownServerState {
    last_state: WorldPayload,
    previous_state: WorldPayload,
}

type ClientResult = Result<(), Box<dyn Error + Send + Sync>>;
type SharedCell<T> = Arc<Mutex<T>>;

pub fn main() -> ClientResult {
    // Params
    let mut args = std::env::args();
    let name = args.nth(1).unwrap_or_else(|| "bob".to_string());
    let address = args.next().unwrap_or_else(|| "127.0.0.1:4000".to_string());

    // Socket
    let socket = Arc::new(UdpSocket::bind("127.0.0.1:0")?);
    let local_addr = socket.local_addr()?;
    println!("INFO: Socket bound to {local_addr}");

    socket.connect(&address)?;
    println!("INFO: socket connected to {address}");

    // Channel
    let cell = SharedCell::new(KnownServerState::default().into());

    // Receive thread
    let running = Arc::new(AtomicBool::new(true));
    let rcv_socket = socket.clone();
    let rcv_running = running.clone();
    let recv_cell = cell.clone();
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

            if let Ok(mut state) = recv_cell.lock() {
                if payload.tick > state.last_state.tick {
                    state.previous_state = state.last_state.clone();
                    state.last_state = payload;
                }
            } else {
                break;
            }
        }
        Ok(())
    });

    // Geez
    let (mut ctx, event_loop) = ContextBuilder::new("les_cubes", "pjdevs")
        .window_setup(WindowSetup::default().title("Les Cubes"))
        .window_mode(WindowMode::default().dimensions(WORLD_SIZE.x, WORLD_SIZE.y))
        .build()?;

    let state = MainState::new(&mut ctx, socket, cell, name)?;
    event::run(ctx, event_loop, state)?;

    running.store(false, Ordering::Relaxed);

    rcv_thread
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))?;

    Ok(())
}
