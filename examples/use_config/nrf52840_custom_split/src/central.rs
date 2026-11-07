#![no_std]
#![no_main]

mod transport;

#[rmk::macros::rmk_central]
mod keyboard {
    #[Override(chip_init)]
    fn chip_init() {
        let mut config = embassy_nrf::config::Config::default();
        config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
        config.lfclk_source = embassy_nrf::config::LfclkSource::ExternalXtal;
        let p = embassy_nrf::init(config);

        embassy_nrf::bind_interrupts!(struct RadioIrqs {
            EGU2_SWI2 => yip::central::YipInterruptHandler;
        });
        let device = yip::DeviceContext::new(
            p.RADIO,
            p.TIMER2,
            p.RTC2,
            yip::ppi_channels![
                p.PPI_CH0, p.PPI_CH1, p.PPI_CH2, p.PPI_CH3, p.PPI_CH4, p.PPI_CH5, p.PPI_CH6, p.PPI_CH7,
            ],
            p.PPI_GROUP0,
            p.EGU2,
            RadioIrqs,
            0,
        )
        .with_central_completion_group(p.PPI_GROUP1);

        // Custom codegen polls this future alongside RMK's other tasks.
        let split_task = async move {
            let [session] = yip::central::init::<1>(device);
            yip::central::start();
            let (mut rx, mut tx, connection) = session.split();

            loop {
                connection.wait_connected().await;
                rmk::split::driver::set_peripheral_connected(0, true);
                let manager = rmk::split::driver::PeripheralManager::new(
                    crate::transport::Driver {
                        rx: &mut rx,
                        tx: &mut tx,
                    },
                    0,
                    rmk::split::PeripheralMatrixConfig {
                        rows: 1,
                        cols: 4,
                        row_offset: 1,
                        col_offset: 0,
                    },
                );
                embassy_futures::select::select(connection.wait_disconnected(), manager.run()).await;
                rmk::split::driver::set_peripheral_connected(0, false);
            }
        };
    }
}
