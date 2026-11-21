mod shared;

use std::{
    collections::HashMap,
    fmt::Display,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    num::ParseFloatError,
    time::{Duration, Instant},
};

use ggez::glam::Vec2;

use crate::shared::WORLD_SIZE;

struct Player {
    id: u64,
    name: String,
    position: Vec2,
    direction: Vec2,
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

enum PlayerInputParseError {
    WrongFormat {
        len: usize,
    },
    WrongValues {
        error: ParseFloatError,
        index: usize,
    },
}

impl Display for PlayerInputParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongFormat { len } => {
                write!(f, "Expected 3 spaced args 'name X Y', received {len}")
            }
            Self::WrongValues { error, index } => {
                write!(f, "Could not parse argument {index}: {error}")
            }
        }
    }
}

struct PlayerInputPayload {
    name: String,
    direction: Vec2,
}

impl TryFrom<&str> for PlayerInputPayload {
    type Error = PlayerInputParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let splits: Vec<&str> = value.split_whitespace().collect();

        if splits.len() != 3 {
            return Err(PlayerInputParseError::WrongFormat { len: splits.len() });
        }

        Ok(PlayerInputPayload {
            name: splits[0].to_string(),
            direction: Vec2 {
                x: splits[1]
                    .parse::<f32>()
                    .map_err(|e| PlayerInputParseError::WrongValues { error: e, index: 1 })?,
                y: splits[2]
                    .parse::<f32>()
                    .map_err(|e| PlayerInputParseError::WrongValues { error: e, index: 2 })?,
            },
        })
    }
}

struct World {
    players: HashMap<SocketAddr, Player>,
    next_player_id: u64,
    player_speed: f32,
}

impl World {
    fn new(player_speed: f32) -> Self {
        Self {
            players: HashMap::new(),
            next_player_id: 0,
            player_speed,
        }
    }

    fn update_or_connect_player(&mut self, addr: &SocketAddr, player_input: PlayerInputPayload) {
        let player = self.players.entry(*addr).or_insert_with(|| {
            let id = self.next_player_id;
            self.next_player_id += 1;
            Player {
                id: id,
                name: String::new(),
                position: Vec2::new(100.0, 100.0),
                direction: Vec2::ZERO,
            }
        });
        player.name = player_input.name;
        player.direction = player_input.direction;
    }

    fn disconnect(&mut self, addr: &SocketAddr) {
        self.players.remove(addr);
    }

    fn step(&mut self, dt: f32) {
        for (_, player) in &mut self.players {
            player.position += player.direction.normalize_or_zero() * self.player_speed * dt;
            player.position = Vec2::new(
                player.position.x.rem_euclid(WORLD_SIZE.x),
                player.position.y.rem_euclid(WORLD_SIZE.y),
            );
        }
    }

    fn snapshot(&self) -> String {
        self.players
            .values()
            .map(|p| p.to_string())
            .collect::<Vec<String>>()
            .join("\n")
            + "\n"
    }
}

// struct RunLoop {
//     fixed_update_ms: u64,
//     dt: f32,
// }

// impl RunLoop {
//     fn new(fixed_update_ms: u64) -> Self {
//         Self {
//             fixed_update_ms,
//             dt: 0.0,
//         }
//     }

//     fn run(&mut self, loop_func: impl Fn(&mut World, &UdpSocket)) {}
// }

// trait ServerMessaging {
//     fn send_message_to(&self, addr: &SocketAddr, message: &String) -> io::Result<usize>;
// }

// impl ServerMessaging for UdpSocket {
//     fn rcv_player_input() -> Result<PlayerInputPayload, PlayerInputParseError> {}

//     fn send_message_to(&self, addr: &SocketAddr, message: &String) -> io::Result<usize> {
//         self.send_to(message.as_bytes(), *addr)
//     }
// }

const PLAYER_SPEED: f32 = 250.0;
const TICK_MILLIS: u64 = 16;

fn main() -> std::io::Result<()> {
    // Params
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4000".to_string());

    // Open socket
    let socket = UdpSocket::bind(&address)?;
    let local_addr = socket.local_addr()?;
    println!("Listenning on {local_addr}");

    // Setup
    let mut world = World::new(PLAYER_SPEED);
    let mut buf = [0u8; 128];
    let mut dt: f32 = 0.0;

    // Main loop
    loop {
        let tick_start_time = Instant::now();
        let mut recv_budget = Duration::from_millis(TICK_MILLIS);

        // Receive loop while budget
        while recv_budget > Duration::ZERO {
            let timer = Instant::now();

            // Receive one packet
            socket.set_read_timeout(Some(recv_budget))?;
            let (len, from) = match socket.recv_from(&mut buf) {
                Ok((len, from)) => (len, from),
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            };

            let Ok(message) = str::from_utf8(&buf[..len]) else {
                socket.send_to("INVALID: payload is not UTF-8\n".as_bytes(), from)?;
                world.disconnect(&from);
                continue;
            };

            let player_input = match PlayerInputPayload::try_from(message) {
                Ok(input) => input,
                Err(err) => {
                    socket.send_to(format!("INVALID: {err}\n").as_bytes(), from)?;
                    world.disconnect(&from);
                    continue;
                }
            };

            // Upate player
            world.update_or_connect_player(&from, player_input);

            // Timer
            recv_budget -= timer.elapsed();
        }

        // Step
        world.step(dt);

        // Send
        let snapshot = world.snapshot().into_bytes();
        for (addr, _) in &world.players {
            socket.send_to(&snapshot, addr)?;
        }

        // Delta
        dt = tick_start_time.elapsed().as_secs_f32();
    }
}
