use core::panic;

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use rmk_config::SplitConnection;
use rmk_config::resolved::Hardware;
use rmk_config::resolved::hardware::{
    BoardConfig, ChipModel, ChipSeries, SerialConfig, SplitConfig,
};

pub(crate) fn expand_split_central_config(hardware: &Hardware) -> proc_macro2::TokenStream {
    if let BoardConfig::Split(split_config) = &hardware.board {
        expand_split_communication_config(&hardware.chip, split_config)
    } else {
        quote! {}
    }
}

fn expand_split_communication_config(chip: &ChipModel, split_config: &SplitConfig) -> TokenStream2 {
    match split_config.connection {
        // The BLE transport loads its peripherals' addresses itself.
        SplitConnection::Ble => quote! {},
        SplitConnection::Serial => {
            // We need to initialize serial instance for serial
            let serial_config: Vec<SerialConfig> = split_config
                .central
                .serial
                .clone()
                .expect("central.serial is required");
            expand_serial_init(chip, serial_config)
        }
        SplitConnection::Esb => {
            let device = expand_yip_device(chip, false);
            let links = split_config.peripheral.len();
            assert!(
                (1..=7).contains(&links),
                "YIP split requires 1..=7 peripherals"
            );
            let policy_arg = if crate::codegen::feature::is_feature_enabled(
                &crate::codegen::feature::get_rmk_features(),
                "dfu_split",
            ) {
                quote! { policy: ::rmk::split::central::UpdatePolicy, }
            } else {
                quote! {}
            };
            let policy = if policy_arg.is_empty() {
                quote! {}
            } else {
                quote! { policy, }
            };
            quote! {
                #device
                let yip_sessions = ::yip::central::init::<#links>(yip_device);
                #[::embassy_executor::task(pool_size = #links)]
                async fn yip_session_task(
                    id: usize,
                    session: ::yip::YipSession,
                    matrix: ::rmk::split::PeripheralMatrixConfig,
                    #policy_arg
                ) {
                    ::rmk::split::esb::run_esb_peripheral_manager(id, session, matrix, #policy).await;
                }
            }
        }
    }
}

/// Reserve the radio resources independently of the USB and Embassy time drivers.
pub(super) fn expand_yip_device(chip: &ChipModel, peripheral: bool) -> TokenStream2 {
    let role = if peripheral {
        quote! { peripheral }
    } else {
        quote! { central }
    };
    if chip.chip == "nrf54lm20" {
        let calibration = if peripheral {
            quote! {
                let calibrated = yip_device.calibrate_clock().await;
                debug_assert!(calibrated, "GRTC/HF calibration failed");
            }
        } else {
            quote! {}
        };
        return quote! {
            ::embassy_nrf::bind_interrupts!(struct YipIrqs {
                EGU10 => ::yip::#role::YipInterruptHandler;
            });
            let mut yip_device = ::yip::DeviceContext::new(
                p.RADIO, p.TIMER10, p.GRTC_CH0,
                ::yip::ppi_channels![
                    p.PPI10_CH0, p.PPI10_CH1, p.PPI10_CH2, p.PPI10_CH3,
                    p.PPI10_CH4, p.PPI10_CH5, p.PPI10_CH6, p.PPI10_CH7,
                    p.PPI10_CH8, p.PPI10_CH9, p.PPI10_CH10,
                ],
                ::yip::ppi_channels![p.PPI20_CH0],
                (p.PPIB21_CH0, p.PPIB11_CH0),
                [
                    ::core::convert::Into::into(p.PPI10_GROUP0),
                    ::core::convert::Into::into(p.PPI10_GROUP1),
                    ::core::convert::Into::into(p.PPI10_GROUP2),
                    ::core::convert::Into::into(p.PPI10_GROUP3),
                ],
                YipIrqs, 0,
            );
            #calibration
        };
    }
    assert_eq!(
        chip.chip, "nrf52840",
        "YIP macros support nrf52840 and nrf54lm20"
    );
    let completion_group = if peripheral {
        quote! {}
    } else {
        quote! { .with_central_completion_group(p.PPI_GROUP1) }
    };
    quote! {
        ::embassy_nrf::bind_interrupts!(struct YipIrqs {
            EGU2_SWI2 => ::yip::#role::YipInterruptHandler;
        });
        let yip_device = ::yip::DeviceContext::new(
            p.RADIO, p.TIMER2, p.RTC2,
            ::yip::ppi_channels![
                p.PPI_CH0, p.PPI_CH1, p.PPI_CH2, p.PPI_CH3,
                p.PPI_CH4, p.PPI_CH5, p.PPI_CH6, p.PPI_CH7,
            ],
            p.PPI_GROUP0, p.EGU2, YipIrqs, 0,
        ) #completion_group;
    }
}

pub(crate) fn expand_serial_init(chip: &ChipModel, serial: Vec<SerialConfig>) -> TokenStream2 {
    let mut uart_initializers = proc_macro2::TokenStream::new();
    serial.iter().enumerate().for_each(|(idx, s)| {
        let tx_buf_static = format_ident!("TX_BUF{}", idx);
        let rx_buf_static = format_ident!("RX_BUF{}", idx);
        let tx_buf_name = format_ident!("tx_buf{}", idx);
        let rx_buf_name = format_ident!("rx_buf{}", idx);
        let uart_buf_init = quote! {
            static #tx_buf_static: ::static_cell::StaticCell<[u8; ::rmk::split::SPLIT_MESSAGE_MAX_SIZE]> = ::static_cell::StaticCell::new();
            let #tx_buf_name = &mut #tx_buf_static.init([0_u8; ::rmk::split::SPLIT_MESSAGE_MAX_SIZE])[..];
            static #rx_buf_static: ::static_cell::StaticCell<[u8; ::rmk::split::SPLIT_MESSAGE_MAX_SIZE]> = ::static_cell::StaticCell::new();
            let #rx_buf_name = &mut #rx_buf_static.init([0_u8; ::rmk::split::SPLIT_MESSAGE_MAX_SIZE])[..];
        };
        let uart_init = match chip.series {
            ChipSeries::Rp2040 => {
                let uart_instance = format_ident!("{}", s.instance);
                let uart_name = format_ident!("{}", s.instance.to_lowercase());
                let tx_pin = format_ident!("{}", s.tx_pin);
                let rx_pin = format_ident!("{}", s.rx_pin);
                let irq_name = format_ident!("IrqsUart{}", idx);
                match &s.instance {
                    i if i.starts_with("UART") => {
                        let uart_irq = format_ident!("{}_IRQ", s.instance);
                        quote! {
                            ::embassy_rp::bind_interrupts!(struct #irq_name {
                                #uart_irq => ::embassy_rp::uart::BufferedInterruptHandler<::embassy_rp::peripherals::#uart_instance>;
                            });
                            let #uart_name = ::embassy_rp::uart::BufferedUart::new(
                                p.#uart_instance,
                                p.#tx_pin,
                                p.#rx_pin,
                                #irq_name,
                                #tx_buf_name,
                                #rx_buf_name,
                                ::embassy_rp::uart::Config::default(),
                            );
                        }
                    }
                    i if i.starts_with("PIO") => {
                        let uart_irq = format_ident!("{}_IRQ_0", s.instance);
                        let instance_init = if s.rx_pin.eq(&s.tx_pin) {
                            quote! {
                                let #uart_name = ::rmk::split::rp::uart::BufferedUart::new_half_duplex(
                                    p.#uart_instance,
                                    p.#rx_pin,
                                    #rx_buf_name,
                                    #irq_name,
                                );
                            }
                        } else {
                            quote! {
                                let #uart_name = ::rmk::split::rp::uart::BufferedUart::new_full_duplex(
                                    p.#uart_instance,
                                    p.#tx_pin,
                                    p.#rx_pin,
                                    #tx_buf_name,
                                    #rx_buf_name,
                                    #irq_name,
                                );
                            }
                        };
                        quote! {
                            ::embassy_rp::bind_interrupts!(struct #irq_name {
                                #uart_irq => ::rmk::split::rp::uart::UartInterruptHandler<::embassy_rp::peripherals::#uart_instance>;
                            });
                            #instance_init
                        }
                    }
                    _ => panic!("Serial instance {:?} is not recognised", s.instance),
                }
            }
            _ => panic!("Serial for chip {:?} isn't implemented yet", chip.series),
        };
        uart_initializers.extend(quote! {
            #uart_buf_init
            #uart_init
        });
    });
    uart_initializers
}
