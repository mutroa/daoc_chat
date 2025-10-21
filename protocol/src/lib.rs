use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::Result;

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    Nick(String),
    Say(String),
    Whisper(String, String),
}

pub async fn write_message<W>(writer: &mut W, message: &Message) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let data = bincode::serialize(message)?;
    writer.write_u32(data.len() as u32).await?;
    writer.write_all(&data).await?;
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
