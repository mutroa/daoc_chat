use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::io::{BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use protocol::{read_message, write_message, Message, PromptMode};
use tracing::{error, info};

type ClientWriter = Arc<Mutex<BufWriter<tokio::net::tcp::OwnedWriteHalf>>>;

struct ClientInfo {
    writer: ClientWriter,
    group: Option<String>,
    is_anon: bool,
    blocked_users: HashSet<String>,
}

type Clients = Arc<Mutex<HashMap<String, ClientInfo>>>;

struct GroupInfo {
    owner: String,
    admins: HashSet<String>,
    password: Option<String>,
    banned: HashSet<String>,
    invites: HashSet<String>,
    group_blocked: HashSet<String>,
}

type Groups = Arc<Mutex<HashMap<String, GroupInfo>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let groups: Groups = Arc::new(Mutex::new(HashMap::new()));

    info!("Server started on 127.0.0.1:8080");

    loop {
        let (stream, addr) = listener.accept().await?;
        info!("New client connected: {}", addr);

        let clients = Arc::clone(&clients);
        let groups = Arc::clone(&groups);
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, clients, groups).await {
                error!("Error with client {}: {:?}", addr, e);
            }
        });
    }
}

async fn handle_client(stream: TcpStream, clients: Clients, groups: Groups) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let writer = Arc::new(Mutex::new(BufWriter::new(write_half)));

    let mut username: Option<String> = None;
    let mut group: Option<String> = None;

    {
        let mut w = writer.lock().await;
        write_message(
            &mut *w,
            &server_message("Enter a username to join the chat."),
        )
        .await?;
        write_message(
            &mut *w,
            &Message::Prompt {
                text: "> ".into(),
                mode: PromptMode::PendingUsername,
            },
        )
        .await?;
    }

    loop {
        let msg = match read_message(&mut reader).await {
            Ok(m) => m,
            Err(_) => break,
        };

        if let Some(ref name) = username {
            let current = {
                let map = clients.lock().await;
                map.get(name).and_then(|info| info.group.clone())
            };
            if current != group {
                group = current;
            }
        }

        match msg {
            Message::Username(name) => {
                let (response, accepted) = {
                    let mut map = clients.lock().await;
                    if let Some(current) = &username {
                        if current == &name {
                            ("Username already set.".to_string(), false)
                        } else {
                            (
                                "Username already set. Please reconnect to change it.".to_string(),
                                false,
                            )
                        }
                    } else if map.contains_key(&name) {
                        ("Username already taken".to_string(), false)
                    } else {
                        info!("Username set: {}", name);
                        username = Some(name.clone());
                        group = None;
                        map.insert(
                            name.clone(),
                            ClientInfo {
                                writer: Arc::clone(&writer),
                                group: None,
                                is_anon: false,
                                blocked_users: HashSet::new(),
                            },
                        );
                        (username_accepted_message().to_string(), true)
                    }
                };

                let mut w = writer.lock().await;

                if !response.is_empty() {
                    write_message(&mut *w, &server_message(response)).await?;
                }

                if accepted {
                    write_message(&mut *w, &server_message(help_display())).await?;
                    if let Some(ref name) = username {
                        let mode = PromptMode::None;
                        let prompt = prompt_text(name, &mode);
                        write_message(&mut *w, &Message::Prompt { text: prompt, mode }).await?;
                    }
                } else if username.is_none() {
                    let prompt = Message::Prompt {
                        text: "> ".into(),
                        mode: PromptMode::PendingUsername,
                    };
                    write_message(&mut *w, &prompt).await?;
                }
            }

            Message::CreateGroup(raw_name) => {
                if let Some(ref creator) = username {
                    let group_name = raw_name.trim();
                    if group_name.is_empty() {
                        let mut w = writer.lock().await;
                        write_message(&mut *w, &server_message("Group name cannot be empty."))
                            .await?;
                        continue;
                    }

                    let creation = {
                        let mut guard = groups.lock().await;
                        if guard.contains_key(group_name) {
                            Err("A group with that name already exists.")
                        } else {
                            guard.insert(
                                group_name.to_string(),
                                GroupInfo {
                                    owner: creator.clone(),
                                    admins: HashSet::new(),
                                    password: None,
                                    banned: HashSet::new(),
                                    invites: HashSet::new(),
                                    group_blocked: HashSet::new(),
                                },
                            );
                            Ok(())
                        }
                    };

                    match creation {
                        Ok(()) => {
                            {
                                let mut map = clients.lock().await;
                                if let Some(info) = map.get_mut(creator) {
                                    info.group = Some(group_name.to_string());
                                }
                            }
                            group = Some(group_name.to_string());

                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!(
                                    "Group '{}' created. You are the admin.",
                                    group_name
                                )),
                            )
                            .await?;
                            let mode = PromptMode::Group(group_name.to_string());
                            let prompt = prompt_text(creator, &mode);
                            write_message(&mut *w, &Message::Prompt { text: prompt, mode }).await?;
                        }
                        Err(msg) => {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                        }
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before creating a group."),
                    )
                    .await?;
                }
            }

            Message::Join {
                group: requested,
                password,
            } => {
                let target_group = requested.trim();
                if target_group.is_empty() {
                    let mut w = writer.lock().await;
                    write_message(&mut *w, &server_message("Group name cannot be empty.")).await?;
                    continue;
                }

                if let Some(ref name) = username {
                    let result: Result<(), String> = {
                        let mut guard = groups.lock().await;
                        if let Some(info) = guard.get_mut(target_group) {
                            if info.banned.contains(name) {
                                Err("You are banned from this group.".to_string())
                            } else {
                                let invited = info.invites.remove(name);
                                let admin = is_group_admin(info, name);
                                let password_ok = info
                                    .password
                                    .as_ref()
                                    .map_or(true, |pw| password.as_deref() == Some(pw));
                                if invited || password_ok || admin {
                                    Ok(())
                                } else if info.password.is_some() {
                                    Err("Incorrect password.".to_string())
                                } else {
                                    Err("You must be invited to join this group.".to_string())
                                }
                            }
                        } else {
                            Err("Group does not exist. Create it with /create group <name>."
                                .to_string())
                        }
                    };

                    match result {
                        Ok(()) => {
                            {
                                let mut map = clients.lock().await;
                                if let Some(info) = map.get_mut(name) {
                                    info.group = Some(target_group.to_string());
                                }
                            }
                            group = Some(target_group.to_string());

                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!("Joined group '{}'.", target_group)),
                            )
                            .await?;
                            let mode = PromptMode::Group(target_group.to_string());
                            let prompt = prompt_text(name, &mode);
                            write_message(&mut *w, &Message::Prompt { text: prompt, mode }).await?;

                            let announcement =
                                server_message(format!("{} joined '{}'.", name, target_group));
                            broadcast_to_group(&clients, name, target_group, &announcement).await?;
                        }
                        Err(msg) => {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                        }
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before joining a group."),
                    )
                    .await?;
                }
            }

            Message::Invite(target) => {
                if let Some(ref inviter) = username {
                    if let Some(ref current_group) = group {
                        let target = target.trim();
                        if target.is_empty() {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("Provide a username to invite."),
                            )
                            .await?;
                            continue;
                        }

                        let exists = {
                            let mut guard = groups.lock().await;
                            guard.get_mut(current_group).map(|info| {
                                info.invites.insert(target.to_string());
                                info.banned.remove(target);
                            })
                        };

                        if exists.is_some() {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!(
                                    "Invited '{}' to join '{}'.",
                                    target, current_group
                                )),
                            )
                            .await?;

                            if let Some(target_writer) = {
                                let map = clients.lock().await;
                                map.get(target).map(|info| Arc::clone(&info.writer))
                            } {
                                let mut tw = target_writer.lock().await;
                                write_message(
                                    &mut *tw,
                                    &server_message(format!(
                                        "{} invited you to join '{}'. Use /join {} to accept.",
                                        inviter, current_group, current_group
                                    )),
                                )
                                .await?;
                            }
                        } else {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("Current group no longer exists."),
                            )
                            .await?;
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("Join a group before inviting others."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before inviting others."),
                    )
                    .await?;
                }
            }

            Message::Promote(target) => {
                if let Some(ref admin) = username {
                    if let Some(ref current_group) = group {
                        let target = target.trim();
                        if target.is_empty() {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("Provide a username to promote."),
                            )
                            .await?;
                            continue;
                        }

                        let target_writer = {
                            let map = clients.lock().await;
                            map.get(target)
                                .filter(|info| info.group.as_deref() == Some(current_group))
                                .map(|info| Arc::clone(&info.writer))
                        };

                        if target_writer.is_none() {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("User must be in the group to be promoted."),
                            )
                            .await?;
                            continue;
                        }

                        let mut message: Option<&str> = None;
                        {
                            let mut guard = groups.lock().await;
                            match guard.get_mut(current_group) {
                                Some(info) => {
                                    if info.owner != *admin {
                                        message = Some("Only the group owner can promote members.");
                                    } else if info.owner == target {
                                        message = Some("Owner has full privileges already.");
                                    } else if info.admins.contains(target) {
                                        message = Some("User is already an admin.");
                                    } else {
                                        info.admins.insert(target.to_string());
                                    }
                                }
                                None => {
                                    message = Some("Current group no longer exists.");
                                }
                            }
                        }

                        if let Some(msg) = message {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                        } else {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!(
                                    "'{}' has been promoted to admin.",
                                    target
                                )),
                            )
                            .await?;

                            if let Some(writer) = target_writer {
                                let mut tw = writer.lock().await;
                                write_message(
                                    &mut *tw,
                                    &server_message(format!(
                                        "You have been promoted to admin of '{}'.",
                                        current_group
                                    )),
                                )
                                .await?;
                            }
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("You must be in a group to promote members."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before promoting users."),
                    )
                    .await?;
                }
            }

            Message::Demote(target) => {
                if let Some(ref owner) = username {
                    if let Some(ref current_group) = group {
                        let target = target.trim();
                        if target.is_empty() {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("Provide a username to demote."),
                            )
                            .await?;
                            continue;
                        }

                        let target_writer = {
                            let map = clients.lock().await;
                            map.get(target)
                                .filter(|info| info.group.as_deref() == Some(current_group))
                                .map(|info| Arc::clone(&info.writer))
                        };

                        let mut message: Option<&str> = None;
                        {
                            let mut guard = groups.lock().await;
                            match guard.get_mut(current_group) {
                                Some(info) => {
                                    if info.owner != *owner {
                                        message = Some("Only the group owner can demote admins.");
                                    } else if info.owner == target {
                                        message = Some("Owner cannot be demoted.");
                                    } else if !info.admins.remove(target) {
                                        message = Some("User is not an admin.");
                                    }
                                }
                                None => message = Some("Current group no longer exists."),
                            }
                        }

                        if let Some(msg) = message {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                        } else {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!("'{}' has been demoted.", target)),
                            )
                            .await?;

                            if let Some(writer) = target_writer {
                                let mut tw = writer.lock().await;
                                write_message(
                                    &mut *tw,
                                    &server_message(format!(
                                        "You have been demoted from admin in '{}'.",
                                        current_group
                                    )),
                                )
                                .await?;
                            }
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("You must be in a group to demote members."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before demoting users."),
                    )
                    .await?;
                }
            }

            Message::Kick(target) => {
                if let Some(ref admin) = username {
                    if let Some(ref current_group) = group {
                        let target = target.trim();
                        if target.is_empty() {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message("Provide a username to kick."))
                                .await?;
                            continue;
                        }

                        if target == admin {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message("You cannot kick yourself."))
                                .await?;
                            continue;
                        }

                        let mut error: Option<&str> = None;
                        {
                            let guard = groups.lock().await;
                            match guard.get(current_group) {
                                Some(info) => {
                                    if !is_group_admin(info, admin) {
                                        error = Some("Only a group admin can kick members.");
                                    } else if info.owner == target {
                                        error = Some("Owner cannot be kicked.");
                                    }
                                }
                                None => error = Some("Current group no longer exists."),
                            }
                        }

                        if let Some(msg) = error {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                            continue;
                        }

                        let mut target_writer: Option<ClientWriter> = None;
                        {
                            let mut map = clients.lock().await;
                            if let Some(info) = map.get_mut(target) {
                                if info.group.as_deref() == Some(current_group) {
                                    info.group = None;
                                    target_writer = Some(Arc::clone(&info.writer));
                                } else {
                                    error = Some("User is not in this group.");
                                }
                            } else {
                                error = Some("User not found.");
                            }
                        }

                        if let Some(msg) = error {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                        } else {
                            {
                                let mut guard = groups.lock().await;
                                if let Some(info) = guard.get_mut(current_group) {
                                    info.admins.remove(target);
                                }
                            }

                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!(
                                    "'{}' has been kicked from '{}'.",
                                    target, current_group
                                )),
                            )
                            .await?;

                            if let Some(writer) = target_writer {
                                let mut tw = writer.lock().await;
                                write_message(
                                    &mut *tw,
                                    &server_message(format!(
                                        "You have been removed from '{}'.",
                                        current_group
                                    )),
                                )
                                .await?;
                                let prompt = prompt_text(&target, &PromptMode::None);
                                write_message(
                                    &mut *tw,
                                    &Message::Prompt {
                                        text: prompt,
                                        mode: PromptMode::None,
                                    },
                                )
                                .await?;
                            }

                            let notice = server_message(format!(
                                "{} was removed from '{}'.",
                                target, current_group
                            ));
                            broadcast_to_group(&clients, admin, current_group, &notice).await?;
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("You must be in a group to kick members."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before kicking users."),
                    )
                    .await?;
                }
            }

            Message::Ban(target) => {
                if let Some(ref admin) = username {
                    if let Some(ref current_group) = group {
                        let target = target.trim();
                        if target.is_empty() {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message("Provide a username to ban."))
                                .await?;
                            continue;
                        }

                        let mut target_writer: Option<ClientWriter> = None;
                        let mut error: Option<&str> = None;

                        {
                            let mut guard = groups.lock().await;
                            match guard.get_mut(current_group) {
                                Some(info) => {
                                    if !is_group_admin(info, admin) {
                                        error = Some("Only a group admin can ban members.");
                                    } else if info.owner == target {
                                        error = Some("Owner cannot be banned.");
                                    } else {
                                        info.admins.remove(target);
                                        info.banned.insert(target.to_string());
                                        info.invites.remove(target);
                                    }
                                }
                                None => error = Some("Current group no longer exists."),
                            }
                        }

                        if let Some(msg) = error {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                            continue;
                        }

                        {
                            let mut map = clients.lock().await;
                            if let Some(info) = map.get_mut(target) {
                                if info.group.as_deref() == Some(current_group) {
                                    info.group = None;
                                    target_writer = Some(Arc::clone(&info.writer));
                                }
                            }
                        }

                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message(format!(
                                "'{}' has been banned from '{}'.",
                                target, current_group
                            )),
                        )
                        .await?;

                        if let Some(writer) = target_writer {
                            let mut tw = writer.lock().await;
                            write_message(
                                &mut *tw,
                                &server_message(format!(
                                    "You have been banned from '{}'.",
                                    current_group
                                )),
                            )
                            .await?;
                            let prompt = prompt_text(&target, &PromptMode::None);
                            write_message(
                                &mut *tw,
                                &Message::Prompt {
                                    text: prompt,
                                    mode: PromptMode::None,
                                },
                            )
                            .await?;
                        }

                        let notice = server_message(format!(
                            "{} was banned from '{}'.",
                            target, current_group
                        ));
                        broadcast_to_group(&clients, admin, current_group, &notice).await?;
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("You must be in a group to ban members."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before banning users."),
                    )
                    .await?;
                }
            }

            Message::Unban(target) => {
                if let Some(ref admin) = username {
                    if let Some(ref current_group) = group {
                        let mut error: Option<&str> = None;
                        {
                            let mut guard = groups.lock().await;
                            match guard.get_mut(current_group) {
                                Some(info) => {
                                    if !is_group_admin(info, admin) {
                                        error = Some("Only a group admin can unban members.");
                                    } else {
                                        info.banned.remove(&target);
                                    }
                                }
                                None => error = Some("Current group no longer exists."),
                            }
                        }

                        let mut w = writer.lock().await;
                        let success = error.is_none();
                        let message = error.unwrap_or("User unbanned.");
                        write_message(&mut *w, &server_message(message)).await?;
                        drop(w);
                        if success {
                            if let Some(writer) = {
                                let map = clients.lock().await;
                                map.get(&target).map(|info| Arc::clone(&info.writer))
                            } {
                                let mut tw = writer.lock().await;
                                write_message(
                                    &mut *tw,
                                    &server_message(format!(
                                        "You have been unbanned from '{}'.",
                                        current_group
                                    )),
                                )
                                .await?;
                                let prompt = prompt_text(&target, &PromptMode::None);
                                write_message(
                                    &mut *tw,
                                    &Message::Prompt {
                                        text: prompt,
                                        mode: PromptMode::None,
                                    },
                                )
                                .await?;
                            }
                            let notice = server_message(format!(
                                "{} was unbanned in '{}'.",
                                target, current_group
                            ));
                            broadcast_to_group(&clients, admin, current_group, &notice).await?;
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("You must be in a group to unban members."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before unbanning users."),
                    )
                    .await?;
                }
            }

            Message::Block(target) => {
                if let Some(ref name) = username {
                    let target = target.trim();
                    if target.is_empty() {
                        let mut w = writer.lock().await;
                        write_message(&mut *w, &server_message("Provide a username to block."))
                            .await?;
                        continue;
                    }
                    if target == name {
                        let mut w = writer.lock().await;
                        write_message(&mut *w, &server_message("You cannot block yourself."))
                            .await?;
                        continue;
                    }
                    let (inserted, target_writer) = {
                        let mut map = clients.lock().await;
                        let exists = map.contains_key(target);
                        let inserted = map
                            .get_mut(name)
                            .map(|info| info.blocked_users.insert(target.to_string()))
                            .unwrap_or(false);
                        let writer = if exists {
                            map.get(target).map(|info| Arc::clone(&info.writer))
                        } else {
                            None
                        };
                        (inserted, writer)
                    };
                    let mut w = writer.lock().await;
                    if !inserted {
                        write_message(&mut *w, &server_message("User already blocked.")).await?;
                    } else {
                        write_message(
                            &mut *w,
                            &server_message(format!("You blocked '{}'.", target)),
                        )
                        .await?;
                        if let Some(writer) = target_writer {
                            let mut tw = writer.lock().await;
                            write_message(
                                &mut *tw,
                                &server_message(format!("{} has blocked you.", name)),
                            )
                            .await?;
                        }
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before blocking users."),
                    )
                    .await?;
                }
            }

            Message::Unblock(target) => {
                if let Some(ref name) = username {
                    let target = target.trim();
                    if target.is_empty() {
                        let mut w = writer.lock().await;
                        write_message(&mut *w, &server_message("Provide a username to unblock."))
                            .await?;
                        continue;
                    }
                    let (removed, target_writer) = {
                        let mut map = clients.lock().await;
                        let removed = map
                            .get_mut(name)
                            .map(|info| info.blocked_users.remove(target))
                            .unwrap_or(false);
                        let writer = map.get(target).map(|info| Arc::clone(&info.writer));
                        (removed, writer)
                    };
                    let mut w = writer.lock().await;
                    if !removed {
                        write_message(&mut *w, &server_message("User was not blocked.")).await?;
                    } else {
                        write_message(
                            &mut *w,
                            &server_message(format!("You unblocked '{}'.", target)),
                        )
                        .await?;
                        if let Some(writer) = target_writer {
                            let mut tw = writer.lock().await;
                            write_message(
                                &mut *tw,
                                &server_message(format!("{} has unblocked you.", name)),
                            )
                            .await?;
                        }
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before unblocking users."),
                    )
                    .await?;
                }
            }

            Message::GroupBlock(target) => {
                if let Some(ref admin) = username {
                    if let Some(ref current_group) = group {
                        let target = target.trim();
                        if target.is_empty() {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message("Provide a username to block."))
                                .await?;
                            continue;
                        }
                        let mut target_writer: Option<ClientWriter> = None;
                        let mut message: Option<&str> = None;
                        {
                            let mut guard = groups.lock().await;
                            match guard.get_mut(current_group) {
                                Some(info) => {
                                    if !is_group_admin(info, admin) {
                                        message = Some(
                                            "Only a group admin can block members from chatting.",
                                        );
                                    } else if info.owner == target {
                                        message = Some("Owner cannot be blocked.");
                                    } else if !info.group_blocked.insert(target.to_string()) {
                                        message = Some("User is already blocked in this group.");
                                    }
                                }
                                None => message = Some("Current group no longer exists."),
                            }
                        }
                        if message.is_none() {
                            let is_member = {
                                let map = clients.lock().await;
                                map.get(target)
                                    .filter(|info| info.group.as_deref() == Some(current_group))
                                    .map(|info| {
                                        target_writer = Some(Arc::clone(&info.writer));
                                        true
                                    })
                                    .unwrap_or(false)
                            };
                            if !is_member {
                                message = Some("User must be in the group to be blocked.");
                                {
                                    let mut guard = groups.lock().await;
                                    if let Some(info) = guard.get_mut(current_group) {
                                        info.group_blocked.remove(target);
                                    }
                                }
                            }
                        }
                        if let Some(msg) = message {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                        } else {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!(
                                    "'{}' has been blocked from speaking in '{}'.",
                                    target, current_group
                                )),
                            )
                            .await?;
                            if let Some(writer) = target_writer {
                                let mut tw = writer.lock().await;
                                write_message(
                                    &mut *tw,
                                    &server_message(format!(
                                        "You have been blocked from speaking in '{}'.",
                                        current_group
                                    )),
                                )
                                .await?;
                            }
                            let notice = server_message(format!(
                                "{} was muted in '{}'.",
                                target, current_group
                            ));
                            broadcast_to_group(&clients, admin, current_group, &notice).await?;
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("Join a group before blocking its members."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before blocking group members."),
                    )
                    .await?;
                }
            }

            Message::GroupUnblock(target) => {
                if let Some(ref admin) = username {
                    if let Some(ref current_group) = group {
                        let target = target.trim();
                        if target.is_empty() {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("Provide a username to unblock."),
                            )
                            .await?;
                            continue;
                        }
                        let mut message: Option<&str> = None;
                        {
                            let mut guard = groups.lock().await;
                            match guard.get_mut(current_group) {
                                Some(info) => {
                                    if !is_group_admin(info, admin) {
                                        message = Some("Only a group admin can unblock members.");
                                    } else if !info.group_blocked.remove(target) {
                                        message = Some("User is not blocked in this group.");
                                    }
                                }
                                None => message = Some("Current group no longer exists."),
                            }
                        }
                        if let Some(msg) = message {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(msg)).await?;
                        } else {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message(format!(
                                    "'{}' has been unblocked in '{}'.",
                                    target, current_group
                                )),
                            )
                            .await?;
                            if let Some(writer) = {
                                let map = clients.lock().await;
                                map.get(target).map(|info| Arc::clone(&info.writer))
                            } {
                                let mut tw = writer.lock().await;
                                write_message(
                                    &mut *tw,
                                    &server_message(format!(
                                        "You can speak again in '{}'.",
                                        current_group
                                    )),
                                )
                                .await?;
                            }
                            let notice = server_message(format!(
                                "{} was unmuted in '{}'.",
                                target, current_group
                            ));
                            broadcast_to_group(&clients, admin, current_group, &notice).await?;
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("Join a group before unblocking its members."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before unblocking group members."),
                    )
                    .await?;
                }
            }

            Message::GroupPassword(password) => {
                if let Some(ref admin) = username {
                    if let Some(ref current_group) = group {
                        let mut error: Option<&str> = None;
                        {
                            let mut guard = groups.lock().await;
                            match guard.get_mut(current_group) {
                                Some(info) => {
                                    if !is_group_admin(info, admin) {
                                        error = Some("Only a group admin can set a password.");
                                    } else {
                                        info.password = password.clone();
                                    }
                                }
                                None => error = Some("Current group no longer exists."),
                            }
                        }

                        let mut w = writer.lock().await;
                        let message = match (error, password) {
                            (Some(msg), _) => msg.to_string(),
                            (None, Some(_)) => format!("Password updated for '{}'.", current_group),
                            (None, None) => format!("Password cleared for '{}'.", current_group),
                        };
                        write_message(&mut *w, &server_message(message)).await?;
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("Join a group before setting a password."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before setting a password."),
                    )
                    .await?;
                }
            }

            Message::GroupSay(text) => {
                if let Some(ref name) = username {
                    if let Some(ref current_group) = group {
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let status = {
                            let guard = groups.lock().await;
                            guard
                                .get(current_group)
                                .map(|info| info.group_blocked.contains(name))
                        };
                        match status {
                            Some(true) => {
                                let mut w = writer.lock().await;
                                write_message(
                                    &mut *w,
                                    &server_message("You are blocked from speaking in this group."),
                                )
                                .await?;
                            }
                            Some(false) => {
                                let formatted = Message::Say(format!(
                                    "[{}@{}]: {}",
                                    name, current_group, trimmed
                                ));
                                {
                                    let mut w = writer.lock().await;
                                    write_message(&mut *w, &formatted).await?;
                                }
                                broadcast_to_group(&clients, name, current_group, &formatted)
                                    .await?;
                            }
                            None => {
                                let mut w = writer.lock().await;
                                write_message(
                                    &mut *w,
                                    &server_message("Current group no longer exists."),
                                )
                                .await?;
                            }
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(&mut *w, &server_message("Join a group before chatting."))
                            .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before chatting."),
                    )
                    .await?;
                }
            }

            Message::GroupWho => {
                if let Some(ref _name) = username {
                    if let Some(ref current_group) = group {
                        let snapshot = {
                            let guard = groups.lock().await;
                            guard
                                .get(current_group)
                                .map(|info| (info.owner.clone(), info.admins.clone()))
                        };
                        if let Some((owner, admins)) = snapshot {
                            let entries = {
                                let map = clients.lock().await;
                                let mut list = Vec::new();
                                for (user, info) in map.iter() {
                                    if info.group.as_deref() == Some(current_group) && !info.is_anon
                                    {
                                        let role = if *user == owner {
                                            "owner"
                                        } else if admins.contains(user) {
                                            "admin"
                                        } else {
                                            "member"
                                        };
                                        list.push(format!("{} {}", user, role));
                                    }
                                }
                                list
                            };
                            let text = if entries.is_empty() {
                                format!("No visible members in '{}'.", current_group)
                            } else {
                                format!("Members of '{}': {}", current_group, entries.join(", "))
                            };
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message(text)).await?;
                        } else {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("Current group no longer exists."),
                            )
                            .await?;
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("Join a group before using /group who."),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before using /group who."),
                    )
                    .await?;
                }
            }

            Message::Who => {
                let (visible, is_anon_self) = {
                    let map = clients.lock().await;
                    let list = map
                        .iter()
                        .filter(|(_, info)| !info.is_anon)
                        .map(|(user, _)| user.clone())
                        .collect::<Vec<_>>();
                    let anon_self = username
                        .as_ref()
                        .and_then(|n| map.get(n))
                        .map(|info| info.is_anon)
                        .unwrap_or(false);
                    (list, anon_self)
                };

                let mut w = writer.lock().await;
                if visible.is_empty() {
                    write_message(&mut *w, &server_message("No visible users online.")).await?;
                } else {
                    write_message(
                        &mut *w,
                        &server_message(format!("Online users: {}", visible.join(", "))),
                    )
                    .await?;
                }
                if is_anon_self {
                    write_message(
                        &mut *w,
                        &server_message(
                            "You are currently anonymous; others cannot see or message you.",
                        ),
                    )
                    .await?;
                }
            }

            Message::AnonToggle => {
                if let Some(ref name) = username {
                    let now_anon = {
                        let mut map = clients.lock().await;
                        map.get_mut(name)
                            .map(|info| {
                                info.is_anon = !info.is_anon;
                                info.is_anon
                            })
                            .unwrap_or(false)
                    };
                    let mut w = writer.lock().await;
                    if now_anon {
                        write_message(
                            &mut *w,
                            &server_message(
                                "You are now anonymous. Others cannot see or message you.",
                            ),
                        )
                        .await?;
                    } else {
                        write_message(&mut *w, &server_message("You are visible again.")).await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before toggling anonymity."),
                    )
                    .await?;
                }
            }

            Message::Say(text) => {
                if let Some(ref name) = username {
                    if let Some(ref group_name) = group {
                        let exists = {
                            let guard = groups.lock().await;
                            guard.contains_key(group_name)
                        };
                        if exists {
                            let formatted = format!("[{}@{}]: {}", name, group_name, text);
                            let msg = Message::Say(formatted);
                            broadcast_to_group(&clients, name, group_name, &msg).await?;
                        } else {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("Current group no longer exists."),
                            )
                            .await?;
                        }
                    } else {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message(
                                "Join a group with /join <group> to chat, or use /send <username> <message> for direct messages.",
                            ),
                        )
                        .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before chatting."),
                    )
                    .await?;
                }
            }

            Message::Send(to, text) => {
                if let Some(ref name) = username {
                    if username.as_deref() == Some(to.as_str()) {
                        let mut w = writer.lock().await;
                        write_message(
                            &mut *w,
                            &server_message("You cannot send direct messages to yourself."),
                        )
                        .await?;
                        continue;
                    }

                    let (sender_blocked, target_entry) = {
                        let map = clients.lock().await;
                        let sender_blocked = map
                            .get(name)
                            .map(|info| info.blocked_users.contains(&to))
                            .unwrap_or(false);
                        let entry = map.get(&to).map(|info| {
                            (
                                Arc::clone(&info.writer),
                                info.is_anon,
                                info.blocked_users.contains(name),
                            )
                        });
                        (sender_blocked, entry)
                    };

                    if sender_blocked {
                        let mut w = writer.lock().await;
                        write_message(&mut *w, &server_message("You have blocked this user."))
                            .await?;
                        continue;
                    }

                    if let Some((target, is_anon_target, target_blocked_sender)) = target_entry {
                        if is_anon_target {
                            let mut w = writer.lock().await;
                            write_message(
                                &mut *w,
                                &server_message("User is not accepting direct messages."),
                            )
                            .await?;
                            continue;
                        }
                        if target_blocked_sender {
                            let mut w = writer.lock().await;
                            write_message(&mut *w, &server_message("User has blocked you."))
                                .await?;
                            continue;
                        }
                        let mut w = target.lock().await;
                        let formatted = Message::Say(format!("\n[{}@you]: {}", name, text));
                        write_message(&mut *w, &formatted).await?;
                        drop(w);

                        let mode = PromptMode::Direct(to.clone());
                        let prompt = prompt_text(name, &mode);
                        let mut self_writer = writer.lock().await;
                        write_message(&mut *self_writer, &Message::Prompt { text: prompt, mode })
                            .await?;
                    } else {
                        let mut w = writer.lock().await;
                        write_message(&mut *w, &server_message(format!("User '{}' not found", to)))
                            .await?;
                    }
                } else {
                    let mut w = writer.lock().await;
                    write_message(
                        &mut *w,
                        &server_message("Enter a username before sending direct messages."),
                    )
                    .await?;
                }
            }

            Message::Quit => {
                let mut w = writer.lock().await;
                write_message(&mut *w, &server_message("Disconnecting...")).await?;
                break;
            }

            Message::Help => {
                let mut w = writer.lock().await;
                write_message(&mut *w, &server_message(help_display())).await?;
            }

            Message::Prompt { .. } => {}
        }
    }

    if let Some(name) = username {
        info!("{} disconnected", name);
        clients.lock().await.remove(&name);
    }

    Ok(())
}
fn help_display() -> &'static str {
    "Commands: /create group <name>, /join <group> [password], /invite <username>, /promote <username>, /demote <username>, /group block <username>, /group unblock <username>, /kick <username>, /ban <username>, /unban <username>, /group pw <password>, /group who, /group <message>, /block <username>, /unblock <username>, /who, /anon, /send <username> <message>, /help, /quit."
}

fn username_accepted_message() -> &'static str {
    "Username accepted. Welcome!"
}

fn prompt_text(username: &str, mode: &PromptMode) -> String {
    match mode {
        PromptMode::None => format!("[{}]: ", username),
        PromptMode::Group(group) | PromptMode::Direct(group) => {
            format!("[{}@{}]: ", username, group)
        }
        PromptMode::PendingUsername => "> ".into(),
    }
}

fn server_message<S: Into<String>>(text: S) -> Message {
    Message::Say(format!("[Server@you]: {}", text.into()))
}

fn is_group_admin(info: &GroupInfo, user: &str) -> bool {
    info.owner == user || info.admins.contains(user)
}

async fn broadcast_to_group(
    clients: &Clients,
    sender: &str,
    group: &str,
    msg: &Message,
) -> anyhow::Result<()> {
    let targets: Vec<ClientWriter> = {
        let map = clients.lock().await;
        map.iter()
            .filter_map(|(name, info)| {
                if name != sender && info.group.as_deref() == Some(group) {
                    Some(Arc::clone(&info.writer))
                } else {
                    None
                }
            })
            .collect()
    };

    for writer in targets {
        let mut w = writer.lock().await;
        write_message(&mut *w, msg).await?;
    }
    Ok(())
}


TODO: 1. add command to create password for an already existing group by owner or admins. 2. fix case sensitive names not working in commands. 3. 