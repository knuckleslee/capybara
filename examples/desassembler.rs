//! Disassembles a range of firmware, without running it.
//!
//! Usage: cargo run --release --example desassembler --
//!            <dump.bin> <hex key> <start> [end] [state.tamastate]
//!
//! The call trace narrowed the shutdown to a branch somewhere before
//! 0x100785D2, where the main loop stops polling and starts winding down. This
//! reads the code there directly rather than waiting fifty seconds to reach it.
//!
//! Addresses in the XIP window are read through a mapping the firmware sets up
//! as it boots, so the console is run for a few seconds first: read straight
//! after loading the image, that window is not yet in place and the range comes
//! back as zeros. A state file may be given instead, and skips the wait.
//!
//! Literal pool words are printed as well: a `LDR rN, [pc, #k]` says nothing on
//! its own, and the value it loads is usually the address or the threshold that
//! the comparison turns on.

use capybara::emulator::etat::Instantane;
use capybara::emulator::peripherals::snsys::CYCLES_PAR_SECONDE;
use capybara::emulator::{Disassembler, Machine, StepResult};

fn nombre(s: &str) -> Option<u32> {
    let s = s.trim();
    let sans = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(sans, 16).ok()
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("dump.bin");
    let key = u32::from_str_radix(a.next().expect("hex key").trim_start_matches("0x"), 16).unwrap();
    let debut = nombre(&a.next().expect("start address")).expect("start address in hex");
    let fin = a.next().and_then(|v| nombre(&v)).unwrap_or(debut + 0x80);
    let etat_path = a.next();

    let mut m = Machine::new();
    m.device_key = Some(key);
    m.load_firmware_file(&path).unwrap();
    let mut restaure = false;
    if let Some(p) = etat_path.as_deref() {
        if !p.is_empty() && p != "-" {
            let etat = Instantane::lire(std::path::Path::new(p)).expect("reading the state");
            m.restaurer(&etat);
            restaure = true;
        }
    }
    if !restaure && (0x1000_0000..=0x100F_FFFF).contains(&debut) {
        // Long enough for the boot code to program the execute-in-place window.
        m.is_running = true;
        let but = m.cpu.cycles + CYCLES_PAR_SECONDE as u64 * 3;
        while m.cpu.cycles < but {
            if !matches!(m.run_frame(), StepResult::Ok(_)) {
                break;
            }
        }
        println!("  window set up by three seconds of boot");
    }

    println!("  {debut:#010x} to {fin:#010x}\n");

    let mut adresse = debut & !1;
    while adresse < fin {
        let w1 = m.bus.read_u16(adresse, &mut m.periph, &m.cpu.nvic);
        let w2 = m.bus.read_u16(adresse.wrapping_add(2), &mut m.periph, &m.cpu.nvic);
        let d = Disassembler::disassemble(adresse, &[w1, w2]);

        // A load relative to the program counter names nothing by itself. The
        // word it reaches is what the code will compare or call, so it is read
        // and shown on the same line.
        let mut note = String::new();
        if d.mnemonic.starts_with("LDR") && d.operands.contains("[pc") {
            if let Some(pos) = d.operands.rfind("; 0x") {
                if let Some(cible) = nombre(&d.operands[pos + 2..]) {
                    let v = m.bus.read_u32(cible, &mut m.periph, &m.cpu.nvic);
                    note = format!("   -> {v:#010x}");
                }
            }
        }

        println!(
            "  {:#010x}   {:04X}{}   {:<10} {}{}",
            adresse,
            w1,
            if d.is_32bit { format!(" {w2:04X}") } else { "     ".to_string() },
            d.mnemonic,
            d.operands,
            note
        );
        adresse += if d.is_32bit { 4 } else { 2 };
    }
}
