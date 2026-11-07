# Custom split transport without a wrapper

This example demonstrates the public split APIs and `SplitConnection::Custom`
from RMK PR #1425. It builds central and peripheral firmware for nRF52840 using
YIP directly. It does not depend on `rmk-yip` or replace RMK's entry-point codegen.

## Follow the integration

1. `keyboard.toml` selects `connection = "custom"` and disables BLE.
2. `src/transport.rs` implements `SplitReader` and `SplitWriter` using YIP and
   postcard. RMK still owns the meaning of each `SplitMessage`.
3. Each board's `#[Override(chip_init)]` initializes the chip and radio resources,
   then creates a future named `split_task`. RMK's existing override mechanism
   inserts that function body into the generated initialization scope.
4. The Custom codegen runs `split_task` alongside the other RMK tasks. Do not
   await it in initialization.
5. The future initializes and starts YIP, waits for a connection, and runs
   `PeripheralManager` (central) or `SplitPeripheral` (peripheral). A disconnect
   cancels that service and the loop waits for the next connection. The central
   reports link transitions through `set_peripheral_connected`.

Connection detection and radio setup belong to this example, not RMK. No extra
executor task is needed here: YIP runs its radio through interrupts, and the
connection/service loop is the supplied future. Other transports can compose
their own background futures in `split_task`.

## Build

With this branch checked out, run:

```sh
rustup target add thumbv7em-none-eabihf
cd examples/use_config/nrf52840_custom_split
cargo build --release --locked --bins
```

YIP is pinned to a Git revision; no sibling checkout or absolute dependency path
is needed. The Embassy Git-source patches select the released versions used by
this example. `Cargo.lock` records the dependency versions.

The binaries are `target/thumbv7em-none-eabihf/release/central` and `peripheral`.
Both use the whole flash starting at address zero, without a bootloader offset.

## Hardware and scope

- Central: nRF52840 DK, four active-low buttons on P0.11/P0.12/P0.24/P0.25.
- Peripheral: nRF52840 (for example nice!nano), four switches from
  P0.06/P0.08/P0.17/P0.20 to ground. This image requires a debugger to flash on
  boards normally using a bootloader.
- Both boards need external HF/LF crystals with the supplied configuration.
- YIP uses RADIO, TIMER2, RTC2, EGU2, PPI channels 0–7 and group 0; the central
  also uses PPI group 1. Embassy time uses RTC1.
- The peripheral's 1x4 matrix maps to row 1 of the central keymap, as specified
  in both `keyboard.toml` and `src/central.rs`.

This is a compile-verified integration example, not a hardware-validated product
configuration. It deliberately omits transport sleep policy, DFU, storage, Vial,
Rynk and multiple peripherals. The radio remains running; add a transport-specific
sleep policy for a battery-powered product. `dfu_split` is not supported by this
minimal example's three-argument `PeripheralManager::new` call.
