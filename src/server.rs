use std::net::UdpSocket;

fn main() -> std::io::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:4000".to_string());
    let socket = UdpSocket::bind(&address)?;
    println!("Listenning on {}", socket.local_addr()?);

    let mut buf = [0u8; 128];
    loop {
        let (len, from) = socket.recv_from(&mut buf)?;
        socket.send_to(&buf[..len], from)?;
    }
}
