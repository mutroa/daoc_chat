use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PromptMode {
    None,
    Group(String),
    Direct(String),
    PendingUsername,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    Username(String),
    CreateGroup(String),
    Join {
        group: String,
        password: Option<String>,
    },
    Invite(String),
    Promote(String),
    Kick(String),
    Ban(String),
    Unban(String),
    GroupPassword(Option<String>),
    Demote(String),
    GroupSay(String),
    GroupWho,
    Who,
    AnonToggle,
    Block(String),
    Unblock(String),
    GroupBlock(String),
    GroupUnblock(String),
    Say(String),
    Send(String, String),
    Quit,
    Help,
    Prompt {
        text: String,
        mode: PromptMode,
    },
}

pub async fn write_message<W>(writer: &mut W, message: &Message) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let data = bincode::serialize(message)?;
    writer.write_u32(data.len() as u32).await?;
    writer.write_all(&data).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_message<R>(reader: &mut R) -> Result<Message>
where
    R: AsyncReadExt + Unpin,
{
    let len = reader.read_u32().await?;
    let mut buf = vec![0; len as usize];
    reader.read_exact(&mut buf).await?;
    let msg = bincode::deserialize(&buf)?;
    Ok(msg)
}
