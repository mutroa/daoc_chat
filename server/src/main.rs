use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use tracing::{info, error};
use protocol::{Message, read_message, write_message};

type Clients = Arc<Mutex<HashMap<String, Arc<Mutex<BufWriter<tokio::net::tcp::OwnedWriteHalf>>>>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));

    info!("Server started on 127.0.0.1:8080");

    loop {
        let (stream, addr) = listener.accept().await?;
        info!("New client connected: {}", addr);

        let clients = Arc::clone(&clients);
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, clients).await {
                error!("Error with client {}: {:?}", addr, e);
            }
        });
    }
}

async fn handle_client(stream: TcpStream, clients: Clients) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let writer = Arc::new(Mutex::new(BufWriter::new(write_half)));

    let mut nickname = None;

    loop {
        let msg = match read_message(&mut reader).await {
            Ok(m) => m,
            Err(_) => break,
        };

        match msg {
            Message::Nick(name) => {
                let mut map = clients.lock().await;
                if map.contains_key(&name) {
                    let mut w = writer.lock().await;
                    write_message(&mut *w, &Message::Say("Nickname already taken".into())).await?;
                } else {
                    info!("Nickname set: {}", name);
                    nickname = Some(name.clone());
                    map.insert(name.clone(), Arc::clone(&writer));
                    let mut w = writer.lock().await;
                    write_message(&mut *w, &Message::Say("Nickname accepted".into())).await?;
                }
            }

            Message::Say(text) => {
                if let Some(ref name) = nickname {
                    let msg = Message::Say(format!("[{}]: {}", name, text));
                    broadcast(&clients, name, &msg).await?;
                }
            }

            Message::Whisper(to, text) => {
                if let Some(ref name) = nickname {
                    let msg = Message::Say(format!("[{} -> {}]: {}", name, to, text));
                    let map = clients.lock().await;
                    if let Some(target) = map.get(&to) {
                        let mut w = target.lock().await;
                        write_message(&mut *w, &msg).await?;
                    }
                }
            }
        }
    }

    if let Some(name) = nickname {
        info!("{} disconnected", name);
        clients.lock().await.remove(&name);
    }

    Ok(())
}

async fn broadcast(clients: &Clients, sender: &str, msg: &Message) -> anyhow::Result<()> {
    let map = clients.lock().await;
    for (name, writer) in map.iter() {
        if name != sender {
            let mut w = writer.lock().await;
            write_message(&mut *w, msg).await?;
        }
    }
    Ok(())
}
