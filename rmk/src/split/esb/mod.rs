pub mod sleep;

use embassy_futures::join::join;
use embassy_futures::select::{Either, select};
use yip::{YipReader, YipSession, YipWriter};

use super::driver::{PeripheralManager, SplitDriverError, SplitReader, SplitWriter, set_peripheral_connected};
use super::{PeripheralMatrixConfig, SPLIT_MESSAGE_MAX_SIZE, SplitMessage};

/// Own one central session, recreating the manager on reconnect.
pub async fn run_esb_peripheral_manager(
    id: usize,
    session: YipSession,
    matrix_config: PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")] policy: super::driver::UpdatePolicy,
) {
    let (mut rx, mut tx, conn) = session.split();
    loop {
        conn.wait_connected().await;
        set_peripheral_connected(id, true);
        info!("peripheral {} connected", id);
        let manager = PeripheralManager::new(
            YipSplitDriver::new(&mut rx, &mut tx),
            id,
            matrix_config,
            #[cfg(feature = "dfu_split")]
            policy,
        );
        let result = select(conn.wait_disconnected(), manager.run()).await;
        set_peripheral_connected(id, false);
        match result {
            Either::First(()) => info!("peripheral {} disconnected: YIP link lost", id),
            Either::Second(()) => warn!("peripheral {} manager returned", id),
        }
    }
}

/// Run the peripheral service while its YIP session is connected.
pub async fn run_esb_peripheral(session: YipSession) {
    join(sleep::run_peripheral_sleep_manager(), run_peripheral_session(session)).await;
}

async fn run_peripheral_session(session: YipSession) {
    let (mut rx, mut tx, conn) = session.split();
    loop {
        conn.wait_connected().await;
        info!("central connected");
        let mut peripheral = super::peripheral::SplitPeripheral::new(YipSplitDriver::new(&mut rx, &mut tx));
        select(conn.wait_disconnected(), async {
            loop {
                peripheral.run().await;
            }
        })
        .await;
        info!("central disconnected");
    }
}

pub(crate) struct YipSplitDriver<'a> {
    rx: &'a mut YipReader,
    tx: &'a mut YipWriter,
}

impl<'a> YipSplitDriver<'a> {
    fn new(rx: &'a mut YipReader, tx: &'a mut YipWriter) -> Self {
        Self { rx, tx }
    }
}

impl SplitReader for YipSplitDriver<'_> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        let data = self.rx.read().await;
        let message = postcard::from_bytes(data.as_ref()).map_err(|_| SplitDriverError::DeserializeError)?;
        if matches!(message, SplitMessage::Key(_) | SplitMessage::Pointing(_)) {
            sleep::report_activity();
        }
        Ok(message)
    }
}

impl SplitWriter for YipSplitDriver<'_> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        let mut buf = [0_u8; SPLIT_MESSAGE_MAX_SIZE];
        let bytes = postcard::to_slice(message, &mut buf).map_err(|_| SplitDriverError::SerializeError)?;
        self.tx.write(bytes).await.map_err(|_| SplitDriverError::SendError)?;
        Ok(bytes.len())
    }
}
