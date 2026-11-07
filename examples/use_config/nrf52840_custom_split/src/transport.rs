use rmk::split::driver::{SplitDriverError, SplitReader, SplitWriter};
use rmk::split::{SPLIT_MESSAGE_MAX_SIZE, SplitMessage};
use yip::{YipReader, YipWriter};

// Only message encoding belongs here; each board owns its connection lifecycle.
pub struct Driver<'a> {
    pub rx: &'a mut YipReader,
    pub tx: &'a mut YipWriter,
}

impl SplitReader for Driver<'_> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        let data = self.rx.read().await;
        postcard::from_bytes(data.as_ref()).map_err(|_| SplitDriverError::DeserializeError)
    }
}

impl SplitWriter for Driver<'_> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        let mut buffer = [0; SPLIT_MESSAGE_MAX_SIZE];
        let bytes = postcard::to_slice(message, &mut buffer).map_err(|_| SplitDriverError::SerializeError)?;
        self.tx.write(bytes).await.map_err(|_| SplitDriverError::Disconnected)?;
        Ok(bytes.len())
    }
}
