//! Records everything the core does on its way into the shutdown.
//!
//! Usage: cargo run --release --example trace_probe --
//!            <dump.bin> <hex key> [state.tamastate] [instructions kept]
//!
//! Asking for one range of disassembly at a time has cost several rounds, and
//! twice the answer was wrong because the execute-in-place window had moved
//! between the run and the reading. This records the whole approach instead,
//! from inside the running machine, and writes it to `trace.txt` for analysis.
//!
//! Two passes. The first runs until the core enters the wind-down routine and
//! notes the cycle. The second re-runs and starts recording shortly before that
//! moment, keeping, for every instruction: its address, the two halfwords of
//! its encoding as the window presented them, and whichever registers it
//! changed. A load's changed register is the value it loaded, so the trace says
//! what was read without needing a hook in the bus.
//!
//! The file is self-contained: instruction bytes come from the window as it
//! stood, so no separate disassembly is needed and none can disagree with it.

use capybara::emulator::etat::Instantane;
use capybara::emulator::{Machine, StepResult};
use std::io::Write;

/// Entry of the routine that winds the console down.
const ARRET: u32 = 0x1002_4018;

fn charger(path: &str, key: u32, etat_path: &Option<String>) -> Machine {
    let mut m = Machine::new();
    m.device_key = Some(key);
    m.load_firmware_file(path).unwrap();
    if let Some(p) = etat_path.as_deref() {
        if !p.is_empty() && p != "-" {
            let etat = Instantane::lire(std::path::Path::new(p)).expect("reading the state");
            m.restaurer(&etat);
        }
    }
    m.bus.mmio_trace.enabled = false;
    m.is_running = true;
    m
}

fn registres(m: &Machine) -> [u32; 16] {
    let mut r = [0u32; 16];
    for (i, v) in r.iter_mut().enumerate().take(13) {
        *v = m.cpu.regs.get_reg(i as u8);
    }
    r[13] = m.cpu.regs.get_sp();
    r[14] = m.cpu.regs.lr;
    r[15] = m.cpu.regs.pc;
    r
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("dump.bin");
    let key = u32::from_str_radix(a.next().expect("hex key").trim_start_matches("0x"), 16).unwrap();
    let etat_path = a.next();
    let garder: usize = a.next().and_then(|v| v.parse().ok()).unwrap_or(60_000);

    println!("  first pass, finding when it winds down...");
    let mut m = charger(&path, key, &etat_path);
    let mut vu = None;
    let plafond = m.cpu.cycles + 96_000_000u64 * 180;
    while m.cpu.cycles < plafond {
        if m.cpu.regs.pc == ARRET {
            vu = Some(m.cpu.cycles);
            break;
        }
        if !matches!(m.step(), StepResult::Ok(_)) {
            break;
        }
    }
    let Some(cible) = vu else {
        println!("  it never reached {ARRET:#010x}; nothing to record.");
        return;
    };
    println!("  it winds down at {cible} cycles\n");

    // Enough to hold the whole approach: at roughly one cycle per instruction
    // this is a comfortable margin over the sixty thousand kept by default.
    let depart = cible.saturating_sub(4_000_000);
    println!("  second pass, recording the last {garder} instructions before it...");
    let mut m = charger(&path, key, &etat_path);

    let mut anneau: std::collections::VecDeque<(u32, u16, u16, [u32; 16])> =
        std::collections::VecDeque::with_capacity(garder + 1);
    let mut precedent: Option<[u32; 16]> = None;
    let mut fichier = std::io::BufWriter::new(std::fs::File::create("trace.txt").unwrap());

    while m.cpu.cycles < cible + 200 {
        let pc = m.cpu.regs.pc;
        if m.cpu.cycles >= depart {
            let regs = registres(&m);
            let w1 = m.bus.read_u16(pc, &mut m.periph, &m.cpu.nvic);
            let w2 = m.bus.read_u16(pc.wrapping_add(2), &mut m.periph, &m.cpu.nvic);
            anneau.push_back((pc, w1, w2, regs));
            if anneau.len() > garder {
                anneau.pop_front();
            }
        }
        if pc == ARRET {
            break;
        }
        if !matches!(m.step(), StepResult::Ok(_)) {
            break;
        }
    }

    writeln!(fichier, "# capybara trace, {} instructions", anneau.len()).unwrap();
    writeln!(fichier, "# winds down at cycle {cible}").unwrap();
    writeln!(
        fichier,
        "# columns: address opcode1 opcode2 then the registers this instruction changed"
    )
    .unwrap();
    writeln!(
        fichier,
        "# a load's changed register holds what it read, so no bus hook is needed"
    )
    .unwrap();
    for (pc, w1, w2, regs) in &anneau {
        write!(fichier, "{pc:08x} {w1:04x} {w2:04x}").unwrap();
        if let Some(p) = precedent {
            for i in 0..15 {
                if p[i] != regs[i] {
                    write!(fichier, " r{i}={:x}", regs[i]).unwrap();
                }
            }
        }
        writeln!(fichier).unwrap();
        precedent = Some(*regs);
    }
    // The registers as the wind-down is entered, which name what was compared.
    let r = registres(&m);
    write!(fichier, "# registers on entry:").unwrap();
    for (i, v) in r.iter().enumerate() {
        write!(fichier, " r{i}={v:x}").unwrap();
    }
    writeln!(fichier).unwrap();
    fichier.flush().unwrap();

    println!("  wrote trace.txt, {} instructions", anneau.len());
    println!("  the instruction bytes are the ones the window presented, so the file");
    println!("  can be read on its own without risk of disagreeing with a later dump.");
}
