#![no_main]

use libfuzzer_sys::fuzz_target;
use x402_near_facilitator::{config::PaymentIdentifierConfig, protocol::parse_request};

fuzz_target!(|data: &[u8]| {
    // accept_v1 = true exercises the legacy v1 sniff + translation branch as
    // well; v2-shaped inputs take the same path either way.
    let _ = parse_request(data, &PaymentIdentifierConfig::default(), true);
    let _ = parse_request(data, &PaymentIdentifierConfig::default(), false);
});
