use futures::StreamExt;
use jsonrpsee::core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};
use tokio::{
    io::AsyncWriteExt,
    process::{ChildStdin, ChildStdout},
};
use tokio_util::codec::{FramedRead, LinesCodec};

/// Create new transport channels using stdin and stdout of a child process.
pub(crate) fn stdio_transport(stdin: ChildStdin, stdout: ChildStdout) -> (Sender, Receiver) {
    (
        Sender(stdin),
        Receiver(FramedRead::new(stdout, LinesCodec::new())),
    )
}

pub(crate) struct Sender(ChildStdin);

#[jsonrpsee::core::async_trait]
impl TransportSenderT for Sender {
    type Error = std::io::Error;

    async fn send(&mut self, msg: String) -> Result<(), Self::Error> {
        let mut sanitized = msg.replace('\n', "");
        sanitized.push('\n');
        let _n = self.0.write_all(sanitized.as_bytes()).await?;
        Ok(())
    }
}

impl From<ChildStdin> for Sender {
    fn from(value: ChildStdin) -> Self {
        Self(value)
    }
}

pub(crate) struct Receiver(FramedRead<ChildStdout, LinesCodec>);

#[jsonrpsee::core::async_trait]
impl TransportReceiverT for Receiver {
    type Error = std::io::Error;

    async fn receive(&mut self) -> Result<ReceivedMessage, Self::Error> {
        let response = self
            .0
            .next()
            .await
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "EOF"))?
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(ReceivedMessage::Text(response))
    }
}

impl From<ChildStdout> for Receiver {
    fn from(value: ChildStdout) -> Self {
        Self(FramedRead::new(value, LinesCodec::new()))
    }
}
