#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: payment_rails_fuzz::WaterfallInput| {
    let _ = payment_rails_fuzz::check_waterfall(input);
});
