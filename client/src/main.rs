use std::io::{stdin, stdout, BufRead, Write};
use std::sync::Arc;

use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

use protocol::{read_message, write_message, Message, PromptMode};
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let stream = TcpStream::connect("127.0.0.1:8080").await?;
    info!("Connected to server.");

    let (read_half, write_half) = stream.into_split();
    let reader = Arc::new(Mutex::new(BufReader::new(read_half)));
    let writer = Arc::new(Mutex::new(BufWriter::new(write_half)));

    let prompt = Arc::new(Mutex::new(String::new()));
    let destination = Arc::new(Mutex::new(PromptMode::PendingUsername));
    let current_user = Arc::new(Mutex::new(None::<String>));
    let current_group = Arc::new(Mutex::new(None::<String>));
    let stdout_lock = Arc::new(Mutex::new(()));

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
    let prompt_clone = Arc::clone(&prompt);
    let destination_clone = Arc::clone(&destination);
    let current_user_clone = Arc::clone(&current_user);
    let current_group_clone = Arc::clone(&current_group);
    let stdout_clone = Arc::clone(&stdout_lock);
    tokio::spawn(async move {
        while let Some(input) = rx.recv().await {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                show_prompt(&prompt_clone, &stdout_clone).await;
                continue;
            }

            let msg_opt = if let Some(rest) = trimmed.strip_prefix("/username ") {
                if matches!(*destination_clone.lock().await, PromptMode::PendingUsername) {
                    let mut prompt_guard = prompt_clone.lock().await;
                    *prompt_guard = String::new();
                }
                Some(Message::Username(rest.to_string()))
            } else if let Some(rest) = trimmed.strip_prefix("/create ") {
                let rest = rest.trim();
                if let Some(group_name) = rest.strip_prefix("group ") {
                    let name = group_name.trim();
                    if name.is_empty() {
                        {
                            let _out = stdout_clone.lock().await;
                            println!("Usage: /create group <group>");
                        }
                        show_prompt(&prompt_clone, &stdout_clone).await;
                        None
                    } else {
                        {
                            let mut prompt_guard = prompt_clone.lock().await;
                            *prompt_guard = String::new();
                        }
                        {
                            let mut dest_guard = destination_clone.lock().await;
                            *dest_guard = PromptMode::None;
                        }
                        {
                            let mut group_guard = current_group_clone.lock().await;
                            *group_guard = None;
                        }
                        Some(Message::CreateGroup(name.to_string()))
                    }
                } else {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /create group <group>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                }
            } else if let Some(rest) = trimmed.strip_prefix("/join ") {
                let mut parts = rest.splitn(2, ' ');
                let group = parts.next().unwrap_or("").trim();
                if group.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /join <group> [password]");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    let password = parts
                        .next()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    {
                        let mut prompt_guard = prompt_clone.lock().await;
                        *prompt_guard = String::new();
                    }
                    {
                        let mut dest_guard = destination_clone.lock().await;
                        *dest_guard = PromptMode::None;
                    }
                    {
                        let mut group_guard = current_group_clone.lock().await;
                        *group_guard = None;
                    }
                    Some(Message::Join {
                        group: group.to_string(),
                        password,
                    })
                }
            } else if let Some(rest) = trimmed.strip_prefix("/invite ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /invite <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Invite(user.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("/promote ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /promote <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Promote(user.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("/demote ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /demote <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Demote(user.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("/kick ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /kick <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Kick(user.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("/ban ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /ban <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Ban(user.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("/unban ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /unban <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Unban(user.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("/group block ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /group block <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::GroupBlock(user.to_string()))
                }
            } else if trimmed == "/group block" {
                {
                    let _out = stdout_clone.lock().await;
                    println!("Usage: /group block <username>");
                }
                show_prompt(&prompt_clone, &stdout_clone).await;
                None
            } else if let Some(rest) = trimmed.strip_prefix("/group unblock ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /group unblock <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::GroupUnblock(user.to_string()))
                }
            } else if trimmed == "/group unblock" {
                {
                    let _out = stdout_clone.lock().await;
                    println!("Usage: /group unblock <username>");
                }
                show_prompt(&prompt_clone, &stdout_clone).await;
                None
            } else if let Some(rest) = trimmed.strip_prefix("/group pw") {
                let password = rest.trim();
                if password.is_empty() {
                    Some(Message::GroupPassword(None))
                } else {
                    Some(Message::GroupPassword(Some(password.to_string())))
                }
            } else if trimmed == "/group who" {
                Some(Message::GroupWho)
            } else if let Some(rest) = trimmed.strip_prefix("/group ") {
                let text = rest.trim();
                if text.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /group <message>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    let current_group = { current_group_clone.lock().await.clone() };
                    if let Some(group_name) = current_group {
                        if let Some(username) = { current_user_clone.lock().await.clone() } {
                            {
                                let mut dest_guard = destination_clone.lock().await;
                                *dest_guard = PromptMode::Group(group_name.clone());
                            }
                            {
                                let mut prompt_guard = prompt_clone.lock().await;
                                *prompt_guard = format!("[{}@{}]: ", username, group_name);
                            }
                        }
                        Some(Message::GroupSay(text.to_string()))
                    } else {
                        {
                            let _out = stdout_clone.lock().await;
                            println!("Join a group before using /group <message>.");
                        }
                        show_prompt(&prompt_clone, &stdout_clone).await;
                        None
                    }
                }
            } else if let Some(rest) = trimmed.strip_prefix("/block ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /block <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Block(user.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("/unblock ") {
                let user = rest.trim();
                if user.is_empty() {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /unblock <username>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                } else {
                    Some(Message::Unblock(user.to_string()))
                }
            } else if trimmed == "/who" {
                Some(Message::Who)
            } else if trimmed == "/anon" {
                Some(Message::AnonToggle)
            } else if let Some(rest) = trimmed.strip_prefix("/send ") {
                if let Some((to, text)) = rest.split_once(' ') {
                    Some(Message::Send(to.to_string(), text.to_string()))
                } else {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("Usage: /send <username> <message>");
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                    None
                }
            } else if trimmed == "/quit" {
                Some(Message::Quit)
            } else if trimmed == "/help" {
                Some(Message::Help)
            } else {
                let dest_snapshot = { destination_clone.lock().await.clone() };
                match dest_snapshot {
                    PromptMode::Group(_) => Some(Message::Say(trimmed.to_string())),
                    PromptMode::Direct(user) => Some(Message::Send(user, trimmed.to_string())),
                    PromptMode::PendingUsername => {
                        {
                            let mut prompt_guard = prompt_clone.lock().await;
                            *prompt_guard = String::new();
                        }
                        Some(Message::Username(trimmed.to_string()))
                    }
                    PromptMode::None => {
                        {
                            let _out = stdout_clone.lock().await;
                            println!("Set a target first using /join <group> or /send <username> <message>.");
                        }
                        show_prompt(&prompt_clone, &stdout_clone).await;
                        None
                    }
                }
            };

            let msg = match msg_opt {
                Some(m) => m,
                None => continue,
            };

            let refresh_after_send = matches!(msg, Message::Say(_) | Message::GroupSay(_));

            {
                let mut writer = writer_clone.lock().await;
                if let Err(e) = write_message(&mut *writer, &msg).await {
                    error!("Send error: {:?}", e);
                    break;
                }
            }

            if let Message::Send(ref to, ref text) = msg {
                let username_opt = {
                    let stored = current_user_clone.lock().await.clone();
                    if stored.is_some() {
                        stored
                    } else {
                        let prompt_snapshot = { prompt_clone.lock().await.clone() };
                        extract_username(&prompt_snapshot)
                    }
                };

                if let Some(username) = username_opt {
                    {
                        let _out = stdout_clone.lock().await;
                        println!("[{}@{}]: {}", username, to, text);
                    }
                    {
                        let mut dest_guard = destination_clone.lock().await;
                        *dest_guard = PromptMode::Direct(to.to_string());
                    }
                    {
                        let mut prompt_guard = prompt_clone.lock().await;
                        *prompt_guard = format!("[{}@{}]: ", username, to);
                    }
                    show_prompt(&prompt_clone, &stdout_clone).await;
                }
            }

            if matches!(msg, Message::Quit) {
                break;
            }

            if refresh_after_send {
                show_prompt(&prompt_clone, &stdout_clone).await;
            }
        }
    });

    // Async receive loop (messages from server)
    let current_user_recv = Arc::clone(&current_user);
    let current_group_recv = Arc::clone(&current_group);
    loop {
        let message = {
            let mut r = reader.lock().await;
            read_message(&mut *r).await
        };

        match message {
            Ok(Message::Say(text)) => {
                {
                    let _out = stdout_lock.lock().await;
                    println!("{}", text);
                }
                show_prompt(&prompt, &stdout_lock).await;
            }
            Ok(Message::Prompt { text: p, mode }) => {
                let (should_print, current_prompt) = {
                    let mut prompt_guard = prompt.lock().await;
                    if *prompt_guard == p {
                        (false, prompt_guard.clone())
                    } else {
                        *prompt_guard = p.clone();
                        (true, prompt_guard.clone())
                    }
                };
                {
                    let mut dest_guard = destination.lock().await;
                    *dest_guard = mode.clone();
                }
                {
                    let mut group_guard = current_group_recv.lock().await;
                    match &mode {
                        PromptMode::Group(g) => *group_guard = Some(g.clone()),
                        PromptMode::None | PromptMode::PendingUsername => *group_guard = None,
                        PromptMode::Direct(_) => {}
                    }
                }
                {
                    let mut user_guard = current_user_recv.lock().await;
                    *user_guard = extract_username(&current_prompt);
                }
                if should_print {
                    show_prompt(&prompt, &stdout_lock).await;
                }
            }
            Ok(_) => {}
            Err(e) => {
                error!("Disconnected from server: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}

async fn show_prompt(prompt: &Arc<Mutex<String>>, stdout_lock: &Arc<Mutex<()>>) {
    let prompt_text = {
        let guard = prompt.lock().await;
        guard.clone()
    };

    if prompt_text.is_empty() {
        return;
    }

    let _out = stdout_lock.lock().await;
    print!("{}", prompt_text);
    let _ = stdout().flush();
}

fn extract_username(prompt: &str) -> Option<String> {
    let start = prompt.find('[')? + 1;
    let end = prompt[start..].find(']')? + start;
    if end <= start {
        return None;
    }
    let inner = &prompt[start..end];
    let username = inner.split('@').next().unwrap_or(inner).trim();
    if username.is_empty() {
        None
    } else {
        Some(username.to_string())
    }
}
