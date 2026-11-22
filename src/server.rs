mod shared;

use std::{
    collections::HashMap,
    error::Error,
    fmt::Display,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::{Mutex, mpsc::sync_channel},
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
    last_ping: Duration,
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
    Encoding,
    Format { len: usize },
    Values { error: String, index: usize },
}

impl Display for PlayerPayloadParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encoding => write!(f, "payload is not UTF-8"),
            Self::Format { len } => write!(
                f,
                "Expected 3 spaced args 'name X Y' or 'bye', received {len} tokens"
            ),
            Self::Values { error, index } => {
                write!(f, "Could not parse token {index}: {error}")
            }
        }
    }
}

#[derive(Debug)]
enum PlayerPayload {
    Bye,
    Input(PlayerInputPayload),
}

#[derive(Debug)]
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
                Err(PlayerPayloadParseError::Values {
                    error: String::from("'bye' expected in a single token command"),
                    index: 0,
                })
            };
        }

        if splits.len() != 3 {
            return Err(PlayerPayloadParseError::Format { len: splits.len() });
        }

        Ok(PlayerPayload::Input(PlayerInputPayload {
            name: splits[0].to_string(),
            direction: Vec2 {
                x: splits[1]
                    .parse::<f32>()
                    .map_err(|e| PlayerPayloadParseError::Values {
                        error: e.to_string(),
                        index: 1,
                    })?,
                y: splits[2]
                    .parse::<f32>()
                    .map_err(|e| PlayerPayloadParseError::Values {
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
    elapsed: Duration,
}

impl World {
    const MAX_PLAYER_NAME_LENGTH: usize = 128;

    fn new(player_speed: f32, players_ttl: Duration) -> Self {
        Self {
            players: HashMap::new(),
            next_player_id: 0,
            player_speed,
            players_ttl,
            ticks: 0,
            elapsed: Duration::ZERO,
        }
    }

    fn update_or_connect_player(&mut self, addr: SocketAddr, payload: PlayerPayload) {
        match payload {
            PlayerPayload::Bye => {
                self.disconnect(addr);
            }
            PlayerPayload::Input(mut player_input_payload) => {
                let safe_name: String = player_input_payload
                    .name
                    .chars()
                    .take(Self::MAX_PLAYER_NAME_LENGTH)
                    .collect();

                if let Some(player) = self.players.get_mut(&addr) {
                    player.name = safe_name;
                    player.direction = player_input_payload.direction;
                    player.last_ping = self.elapsed;
                } else {
                    let step = (self.next_player_id + 1) as f32;
                    let new_player = Player {
                        id: self.next_player_id,
                        name: safe_name,
                        position: Vec2::new(70.0 * step, 100.0 * step),
                        direction: player_input_payload.direction,
                        last_ping: self.elapsed,
                    };
                    self.next_player_id += 1;
                    self.players.insert(addr, new_player);
                    Self::connected(self.players.get(&addr).unwrap());
                }
            }
        };
    }

    fn disconnect(&mut self, addr: SocketAddr) {
        if let Some(player) = self.players.remove(&addr) {
            Self::disconnected(&player);
        }
    }

    fn disconnect_expired_players(&mut self) {
        for (_, player) in self
            .players
            .extract_if(|_, player| self.elapsed - player.last_ping > self.players_ttl)
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

    fn move_players(&mut self, dt: Duration) {
        let distance = self.player_speed * dt.as_secs_f32();
        for player in self.players.values_mut() {
            player.position += player.direction.normalize_or_zero() * distance;
            player.position = Vec2::new(
                player.position.x.rem_euclid(WORLD_SIZE.x),
                player.position.y.rem_euclid(WORLD_SIZE.y),
            );
        }
    }

    fn step(&mut self, dt: Duration) {
        self.elapsed += dt;
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

type MessageParser<T, E> = fn(message: &[u8]) -> Result<T, E>;

fn parse_message(message: &[u8]) -> Result<PlayerPayload, PlayerPayloadParseError> {
    if let Ok(message) = str::from_utf8(message) {
        PlayerPayload::try_from(message)
    } else {
        Err(PlayerPayloadParseError::Encoding)
    }
}

enum Recipients {
    One(SocketAddr),
    Broadcast(usize),
}

struct MessageToSend<T> {
    to: Recipients,
    payload: T,
}

struct MessageBox<T> {
    messages: Vec<MessageToSend<T>>,
    broadcast_recipients: Vec<SocketAddr>,
}

impl<T: Into<String>> MessageBox<T> {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            broadcast_recipients: Vec::new(),
        }
    }

    fn send_to(&mut self, to: SocketAddr, payload: T) {
        self.messages.push(MessageToSend {
            to: Recipients::One(to),
            payload,
        });
    }

    fn broadcast(&mut self, recipients: impl IntoIterator<Item = SocketAddr>, payload: T) {
        let start = self.broadcast_recipients.len();
        self.broadcast_recipients.extend(recipients);
        self.messages.push(MessageToSend {
            to: Recipients::Broadcast(self.broadcast_recipients.len() - start),
            payload,
        });
    }

    fn flush(&mut self, socket: &UdpSocket) {
        let mut recipients = self.broadcast_recipients.drain(..);
        for MessageToSend { to, payload } in self.messages.drain(..) {
            let payload = payload.into();
            match to {
                Recipients::One(addr) => {
                    _ = socket.send_to(payload.as_bytes(), addr);
                }
                Recipients::Broadcast(count) => {
                    for addr in recipients.by_ref().take(count) {
                        _ = socket.send_to(payload.as_bytes(), addr);
                    }
                }
            }
        }
    }
}

trait EventHandler<T, E, P> {
    fn listening(&mut self, local_addr: &SocketAddr);
    fn handle_parse_error(&mut self, from: SocketAddr, error: &E, message_box: &mut MessageBox<P>);
    fn handle_message(&mut self, from: SocketAddr, payload: T, message_box: &mut MessageBox<P>);
    fn fixed_update(&mut self, dt: Duration, message_box: &mut MessageBox<P>);
}

impl EventHandler<PlayerPayload, PlayerPayloadParseError, String> for World {
    fn listening(&mut self, local_addr: &SocketAddr) {
        println!("Listenning on {local_addr}");
    }

    fn handle_parse_error(
        &mut self,
        from: SocketAddr,
        error: &PlayerPayloadParseError,
        message_box: &mut MessageBox<String>,
    ) {
        message_box.send_to(from, format!("INVALID {error}\n"));
    }

    fn handle_message(
        &mut self,
        from: SocketAddr,
        payload: PlayerPayload,
        _message_box: &mut MessageBox<String>,
    ) {
        self.update_or_connect_player(from, payload);
    }

    fn fixed_update(&mut self, dt: Duration, message_box: &mut MessageBox<String>) {
        self.step(dt);
        if !self.players.is_empty() {
            message_box.broadcast(self.players.keys().copied(), self.snapshot());
        }
    }
}

type ServerResult = Result<(), Box<dyn Error + Send + Sync>>;

#[allow(dead_code)]
fn run_udp_event_loop<T, E, P: Into<String>>(
    address: &String,
    fixed_update: Duration,
    parser: MessageParser<T, E>,
    handler: &mut impl EventHandler<T, E, P>,
) -> ServerResult {
    // Open socket
    let socket = UdpSocket::bind(address)?;
    let local_addr = socket.local_addr()?;

    handler.listening(&local_addr);

    // Loop variables
    let mut buffer = [0u8; 128];
    let mut dt = Duration::ZERO;
    let mut message_box = MessageBox::new();

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

            // Parse
            let result = parser(&buffer[..len]);

            // Handle
            match result {
                Ok(message) => handler.handle_message(from, message, &mut message_box),
                Err(error) => handler.handle_parse_error(from, &error, &mut message_box),
            };

            // Timer
            let elapsed = timer.elapsed();
            recv_budget = if elapsed > recv_budget {
                Duration::ZERO
            } else {
                recv_budget - elapsed
            }
        }

        // Step
        handler.fixed_update(dt, &mut message_box);
        message_box.flush(&socket);

        // Delta
        dt = tick_start_time.elapsed();
    }
}

#[allow(dead_code)]
fn run_mt_udp_event_loop<T, E, P: Into<String> + Send>(
    address: &String,
    fixed_update: Duration,
    parser: MessageParser<T, E>,
    mut handler: impl EventHandler<T, E, P> + Send,
) -> ServerResult {
    // Socket
    let socket = UdpSocket::bind(address)?;
    let local_addr = socket.local_addr()?;

    handler.listening(&local_addr);

    let handler_lock = Mutex::new((handler, MessageBox::new()));

    thread::scope(|scope| {
        // Receive thread: its only way out is an I/O error, carried by `?`
        let recv_thread = scope.spawn(|| -> ServerResult {
            let mut buffer = [0u8; 1024];

            loop {
                let (len, from) = socket.recv_from(&mut buffer)?;
                let result = parser(&buffer[..len]);

                // A poisoned lock means the other thread panicked: panic too
                let mut state = handler_lock.lock().unwrap();
                let (handler, message_box) = &mut *state;

                match result {
                    Ok(message) => handler.handle_message(from, message, message_box),
                    Err(error) => handler.handle_parse_error(from, &error, message_box),
                };
            }
        });

        // Fixed update, on this thread, for as long as the receive thread lives
        let mut dt = Duration::ZERO;

        while !recv_thread.is_finished() {
            let tick_start_time = Instant::now();

            thread::sleep(fixed_update);
            let mut state = handler_lock.lock().unwrap();
            let (handler, message_box) = &mut *state;
            handler.fixed_update(dt, message_box);
            message_box.flush(&socket);

            dt = tick_start_time.elapsed();
        }

        // Hand back the receive thread's error, or its panic
        recv_thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

struct ReceivedMessage<T, E> {
    from: SocketAddr,
    parse_result: Result<T, E>,
}

fn run_channel_udp_event_loop<T: Send, E: Send, P: Into<String>>(
    address: &String,
    fixed_update: Duration,
    max_channel_message_count: usize,
    parser: MessageParser<T, E>,
    mut handler: impl EventHandler<T, E, P>,
) -> ServerResult {
    // Socket
    let socket = UdpSocket::bind(address)?;
    let local_addr = socket.local_addr()?;

    handler.listening(&local_addr);

    let (sender, receiver) = sync_channel(max_channel_message_count);
    let mut message_box = MessageBox::new();

    thread::scope(|scope| {
        // Receive thread: its only way out is an I/O error, carried by `?`
        let recv_thread = scope.spawn(|| -> ServerResult {
            let mut buffer = [0u8; 1024];

            loop {
                let (len, from) = socket.recv_from(&mut buffer)?;
                let parse_result = parser(&buffer[..len]);
                if sender.send(ReceivedMessage { from, parse_result }).is_err() {
                    return Ok(());
                }
            }
        });

        // Fixed update, on this thread, for as long as the receive thread lives
        let mut dt = Duration::ZERO;

        // Loop
        while !recv_thread.is_finished() {
            let tick_start_time = Instant::now();

            // Receive loop
            for message in receiver.try_iter() {
                match message.parse_result {
                    Ok(payload) => handler.handle_message(message.from, payload, &mut message_box),
                    Err(error) => {
                        handler.handle_parse_error(message.from, &error, &mut message_box)
                    }
                }
            }

            // Step
            handler.fixed_update(dt, &mut message_box);
            message_box.flush(&socket);

            // Sleep
            let world_duration = tick_start_time.elapsed();
            if world_duration < fixed_update {
                thread::sleep(fixed_update - world_duration);
            }

            // Delta
            dt = tick_start_time.elapsed();
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
const MAX_CHANNEL_MESSAGE_COUNT: usize = 4096;

fn main() -> ServerResult {
    // Params
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4000".to_string());

    let world = World::new(PLAYER_SPEED, PLAYERS_TTL);
    run_channel_udp_event_loop(
        &address,
        TICK_INTERVAL,
        MAX_CHANNEL_MESSAGE_COUNT,
        parse_message,
        world,
    )
}

#[cfg(test)]
mod tests {
    use ggez::glam::Vec2;

    use crate::{
        EventHandler, MessageBox, PLAYER_SPEED, PLAYERS_TTL, PlayerInputPayload, PlayerPayload,
        PlayerPayloadParseError, Recipients, World, parse_message,
    };
    use std::{
        net::{Ipv4Addr, SocketAddr, SocketAddrV4},
        time::Duration,
    };

    const CLIENT_1: SocketAddr =
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 6345));
    const CLIENT_2: SocketAddr =
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 6543));

    #[test]
    fn test_world_shorten_names() {
        let mut world = World::new(PLAYER_SPEED, PLAYERS_TTL);
        let mut message_box = MessageBox::new();
        let very_long_name = vec!["a"; 2000].join("");

        world.handle_message(
            CLIENT_1,
            PlayerPayload::Input(PlayerInputPayload {
                name: very_long_name.clone(),
                direction: Vec2::ONE,
            }),
            &mut message_box,
        );

        assert!(world.players[&CLIENT_1].name.len() < very_long_name.len());
        assert_eq!(message_box.messages.len(), 0);
    }

    #[test]
    fn test_world_broadcast() {
        let mut world = World::new(PLAYER_SPEED, PLAYERS_TTL);
        let mut message_box = MessageBox::new();

        world.handle_message(
            CLIENT_1,
            PlayerPayload::Input(PlayerInputPayload {
                name: "1".into(),
                direction: Vec2::ONE,
            }),
            &mut message_box,
        );
        world.handle_message(
            CLIENT_2,
            PlayerPayload::Input(PlayerInputPayload {
                name: "2".into(),
                direction: Vec2::X,
            }),
            &mut message_box,
        );

        world.fixed_update(Duration::from_millis(1), &mut message_box);

        assert_eq!(message_box.messages.len(), 1);
        assert!(matches!(
            message_box.messages[0].to,
            Recipients::Broadcast(r) if r == 2
        ))
    }

    #[test]
    fn test_world_forget() {
        let mut world = World::new(PLAYER_SPEED, PLAYERS_TTL);
        let mut message_box = MessageBox::new();

        world.handle_message(
            CLIENT_1,
            PlayerPayload::Input(PlayerInputPayload {
                name: "1".into(),
                direction: Vec2::ONE,
            }),
            &mut message_box,
        );

        world.fixed_update(PLAYERS_TTL + Duration::from_nanos(1), &mut message_box);

        assert_eq!(world.players.len(), 0);
    }

    #[test]
    fn test_parser_invalid_malformed_payload() {
        let message = "alice 1 u".as_bytes();

        let result = parse_message(message);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PlayerPayloadParseError::Values { index: 2, .. }
        ));
    }
}
