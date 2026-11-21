mod world;

use std::{
    collections::HashMap,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    num::ParseFloatError,
    time::{Duration, Instant},
};

use ggez::glam::Vec2;

use crate::world::WORLD_SIZE;

#[derive(Debug)]
struct Player {
    id: u32,
    name: String,
    position: Vec2,
    direction: Vec2,
}

impl ToString for Player {
    fn to_string(&self) -> String {
        let id = self.id;
        let name = &self.name;
        let x = self.position.x;
        let y = self.position.y;
        format!("{id} {name} {x} {y}\n")
    }
}

#[derive(Debug)]
enum PlayerInputParseError {
    WrongFormat {
        len: usize,
    },
    WrongValues {
        error: ParseFloatError,
        index: usize,
    },
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

const PLAYER_SPEED: f32 = 250.0;
const TICK_MILLIS: u64 = 16;

fn main() -> std::io::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4000".to_string());
    let socket = UdpSocket::bind(&address)?;
    let local_addr = socket.local_addr()?;
    println!("Listenning on {local_addr}");

    let mut clients = HashMap::<SocketAddr, Player>::new();
    let mut buf = [0u8; 128];
    let mut next_player_id = 0u32;
    let mut ticks: u64 = 0;
    let mut dt: f32 = 0.0;

    loop {
        let tick_start_time = Instant::now();
        let mut recv_budget = Duration::from_millis(TICK_MILLIS);

        // Receive loop while budget
        while recv_budget > Duration::ZERO {
            // Setup
            let timer = Instant::now();

            // Receive one pack
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
                socket.send_to("INVALID: no utf8\n".as_bytes(), from)?;
                clients.remove(&from);
                continue;
            };

            let player_input = match PlayerInputPayload::try_from(message) {
                Ok(input) => input,
                Err(err) => {
                    socket.send_to(format!("INVALID: {err:?}\n").as_bytes(), from)?;
                    clients.remove(&from);
                    continue;
                }
            };
            let incoming_player = clients.entry(from).or_insert_with(|| {
                let id = next_player_id;
                next_player_id += 1;
                Player {
                    id: id,
                    name: player_input.name.clone(),
                    position: Vec2::new(100.0, 100.0),
                    direction: player_input.direction,
                }
            });

            // Update
            incoming_player.name = player_input.name;
            incoming_player.direction = player_input.direction;

            // Timer
            recv_budget -= timer.elapsed();
        }

        // Step
        for (_, player) in &mut clients {
            player.position += player.direction.normalize_or_zero() * PLAYER_SPEED * dt;
            player.position = Vec2::new(
                player.position.x.rem_euclid(WORLD_SIZE.x),
                player.position.y.rem_euclid(WORLD_SIZE.y),
            );
        }

        // Send
        let world_state: String = clients
            .values()
            .map(|p| p.to_string())
            .collect::<Vec<String>>()
            .join("\n");

        for (client_addr, _) in &clients {
            socket.send_to(world_state.to_string().as_bytes(), client_addr)?;
        }

        ticks += 1;
        dt = tick_start_time.elapsed().as_secs_f32();
    }
}
