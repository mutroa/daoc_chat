use std::io::{stdin, BufRead};
use std::sync::Arc;

use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};

use tracing::{info, error};
use protocol::{Message, read_message, write_message};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let stream = TcpStream::connect("127.0.0.1:8080").await?;
    info!("Connected to server.");

    let (read_half, write_half) = stream.into_split();
    let reader = Arc::new(Mutex::new(BufReader::new(read_half)));
    let writer = Arc::new(Mutex::new(BufWriter::new(write_half)));

    // Create unbounded channel to receive stdin lines
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Spawn blocking thread for stdin
    std::thread::spawn(move || {
        let stdin = stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(input) => {
                    if tx.send(input).is_err() {
                        break; // receiver dropped
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Async task to send messages from user input
    let writer_clone = Arc::clone(&writer);
    tokio::spawn(async move {
        while let Some(input) = rx.recv().await {
            let msg = if input.starts_with("/nick ") {
                Message::Nick(input[6..].to_string())
            } else if input.starts_with("/whisper ") {
                let rest = &input[9..];
                if let Some((to, text)) = rest.split_once(' ') {
                    Message::Whisper(to.to_string(), text.to_string())
                } else {
                    println!("Usage: /whisper <name> <message>");
                    continue;
                }
            } else {
                Message::Say(input)
            };

            let mut writer = writer_clone.lock().await;
            if let Err(e) = write_message(&mut *writer, &msg).await {
                error!("Send error: {:?}", e);
            }
        }
    });

    // Async receive loop (messages from server)
    loop {
        let mut r = reader.lock().await;
        match read_message(&mut *r).await {
            Ok(Message::Say(text)) => println!("{}", text),
            Ok(_) => {}
            Err(e) => {
                error!("Disconnected from server: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}
