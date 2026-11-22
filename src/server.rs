mod shared;

use std::{
    collections::HashMap,
    error::Error,
    fmt::Display,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use ggez::glam::Vec2;

use crate::shared::WORLD_SIZE;

struct Player {
    id: u64,
    name: String,
    position: Vec2,
    direction: Vec2,
    last_ping: Instant,
}

impl Display for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = self.id;
        let name = &self.name;
        let x = self.position.x;
        let y = self.position.y;
        write!(f, "{id} {name} {x} {y}")
    }
}

enum PlayerPayloadParseError {
    WrongFormat { len: usize },
    WrongValues { error: String, index: usize },
}

impl Display for PlayerPayloadParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongFormat { len } => {
                write!(
                    f,
                    "Expected 3 spaced args 'name X Y' or 'bye', received {len} tokens"
                )
            }
            Self::WrongValues { error, index } => {
                write!(f, "Could not parse token {index}: {error}")
            }
        }
    }
}

enum PlayerPayload {
    Bye,
    Input(PlayerInputPayload),
}

struct PlayerInputPayload {
    name: String,
    direction: Vec2,
}

impl TryFrom<&str> for PlayerPayload {
    type Error = PlayerPayloadParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let splits: Vec<&str> = value.split_whitespace().collect();

        if splits.len() == 1 {
            return if splits[0] == "bye" {
                Ok(PlayerPayload::Bye)
            } else {
                Err(PlayerPayloadParseError::WrongValues {
                    error: String::from("'bye' expected in a single token command"),
                    index: 0,
                })
            };
        }

        if splits.len() != 3 {
            return Err(PlayerPayloadParseError::WrongFormat { len: splits.len() });
        }

        Ok(PlayerPayload::Input(PlayerInputPayload {
            name: splits[0].to_string(),
            direction: Vec2 {
                x: splits[1]
                    .parse::<f32>()
                    .map_err(|e| PlayerPayloadParseError::WrongValues {
                        error: e.to_string(),
                        index: 1,
                    })?,
                y: splits[2]
                    .parse::<f32>()
                    .map_err(|e| PlayerPayloadParseError::WrongValues {
                        error: e.to_string(),
                        index: 2,
                    })?,
            },
        }))
    }
}

struct World {
    players: HashMap<SocketAddr, Player>,
    next_player_id: u64,
    player_speed: f32,
    players_ttl: Duration,
    ticks: u64,
}

impl World {
    fn new(player_speed: f32, players_ttl: Duration) -> Self {
        Self {
            players: HashMap::new(),
            next_player_id: 0,
            player_speed,
            players_ttl,
            ticks: 0,
        }
    }

    fn update_or_connect_player(&mut self, addr: &SocketAddr, payload: PlayerPayload) {
        match payload {
            PlayerPayload::Bye => {
                self.disconnect(addr);
            }
            PlayerPayload::Input(player_input_payload) => {
                if let Some(player) = self.players.get_mut(addr) {
                    player.name = player_input_payload.name;
                    player.direction = player_input_payload.direction;
                    player.last_ping = Instant::now();
                } else {
                    let new_player = Player {
                        id: self.next_player_id,
                        name: player_input_payload.name,
                        position: Vec2::new(100.0, 100.0),
                        direction: player_input_payload.direction,
                        last_ping: Instant::now(),
                    };
                    self.next_player_id += 1;
                    self.players.insert(*addr, new_player);
                    Self::connected(self.players.get(addr).unwrap());
                }
            }
        };
    }

    fn disconnect(&mut self, addr: &SocketAddr) {
        if let Some(player) = self.players.remove(addr) {
            Self::disconnected(&player);
        }
    }

    fn disconnect_expired_players(&mut self) {
        for (_, player) in self
            .players
            .extract_if(|_, player| player.last_ping.elapsed() > self.players_ttl)
        {
            Self::disconnected(&player);
        }
    }

    fn connected(player: &Player) {
        println!("INFO: Connected player {player}");
    }

    fn disconnected(player: &Player) {
        println!("INFO: Disconnected player {player}");
    }

    fn move_players(&mut self, dt: f32) {
        for player in self.players.values_mut() {
            player.position += player.direction.normalize_or_zero() * self.player_speed * dt;
            player.position = Vec2::new(
                player.position.x.rem_euclid(WORLD_SIZE.x),
                player.position.y.rem_euclid(WORLD_SIZE.y),
            );
        }
    }

    fn step(&mut self, dt: f32) {
        self.disconnect_expired_players();
        self.move_players(dt);
        self.ticks += 1;
    }

    fn snapshot(&self) -> String {
        format!("{}\n", self.ticks)
            + &self
                .players
                .values()
                .map(|p| p.to_string())
                .collect::<Vec<String>>()
                .join("\n")
            + "\n"
    }
}

trait EventHandler {
    fn listening(&mut self, local_addr: &SocketAddr);
    fn message_received(&mut self, socket: &UdpSocket, from: &SocketAddr, message: &[u8]);
    fn fixed_update(&mut self, socket: &UdpSocket, dt: f32);
}

impl EventHandler for World {
    fn listening(&mut self, local_addr: &SocketAddr) {
        println!("Listenning on {local_addr}");
    }

    fn message_received(&mut self, socket: &UdpSocket, from: &SocketAddr, message: &[u8]) {
        let Ok(message) = str::from_utf8(message) else {
            _ = socket.send_to("INVALID payload is not UTF-8\n".as_bytes(), from);
            self.disconnect(from);
            return;
        };

        let payload = match PlayerPayload::try_from(message) {
            Ok(input) => input,
            Err(err) => {
                _ = socket.send_to(format!("INVALID {err}\n").as_bytes(), from);
                self.disconnect(from);
                return;
            }
        };

        // Upate player
        self.update_or_connect_player(from, payload);
    }

    fn fixed_update(&mut self, socket: &UdpSocket, dt: f32) {
        // Step
        self.step(dt);

        // Send
        let snapshot = self.snapshot().into_bytes();
        for addr in self.players.keys() {
            _ = socket.send_to(&snapshot, addr);
        }
    }
}

type ServerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[allow(dead_code)]
fn run_udp_event_loop(
    address: &String,
    fixed_update: Duration,
    handler: &mut impl EventHandler,
) -> ServerResult {
    // Open socket
    let socket = UdpSocket::bind(address)?;
    let local_addr = socket.local_addr()?;

    handler.listening(&local_addr);

    // Loop variables
    let mut buffer = [0u8; 128];
    let mut dt: f32 = 0.0;

    // Loop
    loop {
        let tick_start_time = Instant::now();
        let mut recv_budget = fixed_update;

        // Receive loop while budget
        while recv_budget > Duration::ZERO {
            let timer = Instant::now();

            // Receive one packet
            socket.set_read_timeout(Some(recv_budget))?;
            let (len, from) = match socket.recv_from(&mut buffer) {
                Ok((len, from)) => (len, from),
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            };

            // Handle
            handler.message_received(&socket, &from, &buffer[..len]);

            // Timer
            let elapsed = timer.elapsed();
            recv_budget = if elapsed > recv_budget {
                Duration::ZERO
            } else {
                recv_budget - elapsed
            }
        }

        // Step
        handler.fixed_update(&socket, dt);

        // Delta
        dt = tick_start_time.elapsed().as_secs_f32();
    }
}

fn run_mt_udp_event_loop(
    address: &String,
    fixed_update: Duration,
    mut handler: impl EventHandler + Send,
) -> ServerResult {
    // Socket
    let socket = UdpSocket::bind(address)?;
    let local_addr = socket.local_addr()?;

    handler.listening(&local_addr);

    let handler_lock = Mutex::new(handler);

    thread::scope(|scope| {
        // Receive thread: its only way out is an I/O error, carried by `?`
        let recv_thread = scope.spawn(|| -> ServerResult {
            let mut buffer = [0u8; 1024];

            loop {
                let (len, from) = socket.recv_from(&mut buffer)?;
                // A poisoned lock means the other thread panicked: panic too
                handler_lock
                    .lock()
                    .unwrap()
                    .message_received(&socket, &from, &buffer[..len]);
            }
        });

        // Fixed update, on this thread, for as long as the receive thread lives
        let mut dt: f32 = 0.0;

        while !recv_thread.is_finished() {
            let start_time = Instant::now();

            thread::sleep(fixed_update);
            handler_lock.lock().unwrap().fixed_update(&socket, dt);

            dt = start_time.elapsed().as_secs_f32();
        }

        // Hand back the receive thread's error, or its panic
        recv_thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

const PLAYER_SPEED: f32 = 250.0;
const PLAYERS_TTL: Duration = Duration::from_millis(5000);
const TICK_INTERVAL: Duration = Duration::from_millis(16);

fn main() -> ServerResult {
    // Params
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4000".to_string());

    let world = World::new(PLAYER_SPEED, PLAYERS_TTL);
    run_mt_udp_event_loop(&address, TICK_INTERVAL, world)
}
