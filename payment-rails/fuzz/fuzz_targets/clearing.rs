#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: payment_rails_fuzz::ClearingInput| {
    let _ = payment_rails_fuzz::check_clearing(input);
});
