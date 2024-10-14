use futures::StreamExt;
use jsonrpsee::core::client::{MaybeSend, ReceivedMessage, TransportReceiverT, TransportSenderT};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    process::{ChildStdin, ChildStdout},
};
use tokio_util::codec::{FramedRead, LinesCodec};

/// Create new transport channels using stdin and stdout of a child process.
pub(crate) fn stdio_transport(
    stdin: ChildStdin,
    stdout: ChildStdout,
) -> (Sender<ChildStdin>, Receiver<ChildStdout>) {
    (
        Sender(stdin),
        Receiver(FramedRead::new(stdout, LinesCodec::new())),
    )
}

pub(crate) struct Sender<T>(T);

#[jsonrpsee::core::async_trait]
impl<T: AsyncWrite + MaybeSend + Unpin + 'static> TransportSenderT for Sender<T> {
    type Error = std::io::Error;

    async fn send(&mut self, msg: String) -> Result<(), Self::Error> {
        let mut sanitized = msg.replace('\n', "");
        sanitized.push('\n');
        let _n = self.0.write_all(sanitized.as_bytes()).await?;
        Ok(())
    }
}

impl<T: AsyncWrite + MaybeSend + Unpin + 'static> From<T> for Sender<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

pub(crate) struct Receiver<T>(FramedRead<T, LinesCodec>);

#[jsonrpsee::core::async_trait]
impl<T: AsyncRead + MaybeSend + Unpin + 'static> TransportReceiverT for Receiver<T> {
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

impl<T: AsyncRead + MaybeSend + Unpin + 'static> From<T> for Receiver<T> {
    fn from(value: T) -> Self {
        Self(FramedRead::new(value, LinesCodec::new()))
    }
}
