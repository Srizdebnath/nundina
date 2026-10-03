#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: payment_rails_fuzz::SettleFillInput| {
    let _ = payment_rails_fuzz::check_settle_fill(input);
});
