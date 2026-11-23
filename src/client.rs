mod shared;

use std::cmp::max;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::{io, thread};

use ggez::conf::{WindowMode, WindowSetup};
use ggez::event::{self, EventHandler};
use ggez::glam::Vec2;
use ggez::graphics::{self, Color, DrawParam, Mesh, PxScale, Rect, Text, TextFragment};
use ggez::winit::keyboard::{KeyCode, PhysicalKey};
use ggez::{Context, ContextBuilder, GameResult};

use crate::shared::{TICK_INTERVAL, WORLD_SIZE};

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

struct YouPayload {
    id: u64,
}

impl TryFrom<&str> for YouPayload {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let id = value
            .strip_prefix("YOU ")
            .ok_or("Expected YOU prefix")?
            .trim()
            .parse()
            .map_err(|_| "Invalid player ID")?;

        Ok(Self { id })
    }
}

struct PlayerState {
    position: Vec2,
    name: String,
    mesh: Mesh,
    name_text: Text,
    color: Color,
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

const PLAYER_SIZE: Vec2 = Vec2::new(50.0, 50.0);
const PLAYER_TEXT_SIZE: f32 = 24.0;
const PLAYER_TEXT_OFFSET: Vec2 = Vec2::new(0.0, -25.0);
const PLAYER_DEFAULT_COLOR: Color = Color::CYAN;
const PLAYER_MY_COLOR: Color = Color::YELLOW;

fn create_player_mesh(ctx: &Context) -> Mesh {
    Mesh::new_rectangle(
        ctx,
        graphics::DrawMode::fill(),
        Rect::new(0.0, 0.0, PLAYER_SIZE.x, PLAYER_SIZE.y),
        Color::WHITE,
    )
    .expect("Should be able to create a mesh here")
}

fn create_player_name_text(name: &str) -> Text {
    Text::new(TextFragment {
        text: name.to_owned(),
        color: Some(Color::WHITE),
        scale: Some(PxScale::from(PLAYER_TEXT_SIZE)),
        ..Default::default()
    })
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
            let player_state = self
                .players
                .entry(player.id)
                .or_insert_with(|| PlayerState {
                    position: player.position,
                    name: player.name.clone(),
                    mesh: create_player_mesh(ctx),
                    name_text: create_player_name_text(&player.name),
                    color: PLAYER_DEFAULT_COLOR,
                });

            if player_state.position.distance_squared(player.position)
                > WORLD_SIZE.max_element().powi(2) / 4.0
            {
                player_state.position = player.position;
            } else {
                let interval = max(
                    TICK_INTERVAL,
                    TICK_INTERVAL * (state.last_state.tick - state.previous_tick) as u32,
                );
                let alpha = (state.last_received_at.elapsed().as_secs_f32()
                    / interval.as_secs_f32())
                .clamp(0.0, 1.0);
                player_state.position = player_state.position.lerp(player.position, alpha);
            }

            if player.name != player_state.name {
                player_state.name = player.name;
                player_state.name_text = create_player_name_text(&player_state.name);
            }

            known_players.insert(player.id);
        }

        self.players
            .retain(|player_id, _| known_players.contains(player_id));

        if state.my_id != self.my_id {
            self.my_id = state.my_id;
            if let Some(id) = self.my_id {
                self.players
                    .entry(id)
                    .and_modify(|me| me.color = PLAYER_MY_COLOR);
            }
        }

        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let mut canvas = graphics::Canvas::from_frame(ctx, self.clear_color);

        for player in self.players.values() {
            canvas.draw(
                &player.mesh,
                DrawParam::default()
                    .dest(player.position)
                    .color(player.color),
            );
            canvas.draw(
                &player.name_text,
                DrawParam::default().dest(player.position + PLAYER_TEXT_OFFSET),
            );
        }

        canvas.finish(ctx)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct KnownServerState {
    my_id: Option<u64>,
    last_state: WorldPayload,
    last_received_at: Instant,
    previous_tick: u64,
}

impl Default for KnownServerState {
    fn default() -> Self {
        Self {
            my_id: None,
            last_state: WorldPayload {
                tick: 0,
                players: Vec::new(),
            },
            last_received_at: Instant::now(),
            previous_tick: 0,
        }
    }
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

            if let Ok(payload) = YouPayload::try_from(message) {
                let Ok(mut state) = recv_cell.lock() else {
                    break;
                };
                state.my_id = Some(payload.id);
                continue;
            }

            let Ok(payload) = WorldPayload::try_from(message) else {
                continue;
            };

            if let Ok(mut state) = recv_cell.lock() {
                if payload.tick > state.last_state.tick {
                    state.previous_tick = state.last_state.tick;
                    state.last_state = payload;
                    state.last_received_at = Instant::now();
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
