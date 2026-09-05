//! Who orders the shutdown, and from where.
//!
//! Usage: cargo run --release --example extinction_probe --
//!            <dump.bin> <hex key> [state.tamastate] [max real seconds]
//!
//! Earlier probes settled what the shutdown is not. It is not the battery — the
//! sample reads full and the converter is running at the moment the decision is
//! taken. It is not the power manager, whose registers are both zero. It is not
//! the fast-forward, since the console sleeps just the same without it. And it
//! is not any of the eleven idle counters or timestamps that matched a
//! plausible signature.
//!
//! Following the calls then showed the shape of the ending: a routine at
//! `0x10024018`, called from `0x1000FECC`, which runs for some ten milliseconds
//! before clearing the interrupts one module at a time, programming the wake
//! alarm through the clock block, switching SysTick off and parking in the loop
//! in PRAM. All of that is consequence. The decision was taken before the call.
//!
//! So the probe now stops at the call itself rather than at its late effects,
//! which leaves the caller's own frame intact and its condition still readable.
//!
//! So this probe stops guessing at variables and follows the calls. It keeps a
//! shadow call stack — every branch-with-link pushed, every return popped — and
//! when the shutdown begins it prints the chain of callers, the raw stack
//! beneath them, and the calls made just before.
//!
//! It also disassembles around each of those call sites, on the spot. Reading
//! them afterwards with a separate tool gave code that did not match: the
//! execute-in-place window is programmable, so the same address holds different
//! instructions depending on how the firmware has arranged it, and a listing
//! taken three seconds after boot says nothing about what ran fifty seconds
//! later. Disassembling here, with the machine still in the state that took the
//! decision, is the only reading that can be trusted.

use capybara::emulator::etat::Instantane;
use capybara::emulator::{Disassembler, Lieu, Machine, StepResult};

/// Calls kept before the trigger.
const APPELS: usize = 120;

/// Entry of the routine that winds the console down. Reaching it is the
/// trigger: by the time it clears an interrupt, ten milliseconds later, the
/// frame that decided has long returned.
///
/// Give another address as the fifth argument to follow a different lead.
const ARRET: u32 = 0x1002_4018;

struct Appel {
    depuis: u32,
    vers: u32,
    retour: u32,
    sp: u32,
    cycles: u64,
}

fn code_plausible(v: u32) -> bool {
    let a = v & !1;
    (v & 1) != 0 && (a <= 0x0000_FFFF || (0x1000_0000..=0x100F_FFFF).contains(&a))
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("dump.bin");
    let key = u32::from_str_radix(a.next().expect("hex key").trim_start_matches("0x"), 16).unwrap();
    let etat_path = a.next();
    let limite: f64 = a.next().and_then(|v| v.parse().ok()).unwrap_or(1800.0);
    let arret = a
        .next()
        .and_then(|v| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
        .unwrap_or(ARRET);

    let mut m = Machine::new();
    m.device_key = Some(key);
    m.load_firmware_file(&path).unwrap();
    if let Some(p) = etat_path.as_deref() {
        if !p.is_empty() && p != "-" {
            let etat = Instantane::lire(std::path::Path::new(p)).expect("reading the state");
            m.restaurer(&etat);
            println!("  starting from {p}");
        }
    }
    // The same addresses the interface watches, so a setting that does not work
    // can be tried here, where the reason is visible. Without this the probe
    // would always be measuring the untreated console.
    let lire_adresses = |nom: &str| -> Vec<(u32, u8)> {
        std::env::var(nom)
            .unwrap_or_default()
            .split(',')
            .filter_map(|e| {
                let e = e.trim();
                if e.is_empty() {
                    return None;
                }
                let (a, l) = e.split_once(':').unwrap_or((e, "2"));
                Some((
                    u32::from_str_radix(a.trim().trim_start_matches("0x"), 16).ok()?,
                    l.trim().parse().ok()?,
                ))
            })
            .collect()
    };
    m.compteur_inactivite = lire_adresses("CAPYBARA_COMPTEUR_INACTIVITE");
    m.horodatage_activite = lire_adresses("CAPYBARA_HORODATAGE_ACTIVITE");
    let hexa = |v: &str| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok();
    m.drapeau_activite = std::env::var("CAPYBARA_DRAPEAU_ACTIVITE")
        .unwrap_or_default()
        .split(',')
        .filter_map(|e| {
            let mut champs = e.trim().split(':');
            let texte = champs.next()?.trim();
            let lieu = if let Some(reste) = texte.strip_prefix('*') {
                let (p, d) = reste.split_once('+').unwrap_or((reste, "0"));
                Lieu::Indirect {
                    pointeur: hexa(p)?,
                    decalage: d.trim().parse().ok().or_else(|| hexa(d))?,
                }
            } else {
                Lieu::Fixe(hexa(texte)?)
            };
            let largeur: u8 = champs.next().unwrap_or("1").trim().parse().ok()?;
            let poser = champs.next().and_then(hexa).unwrap_or(0);
            let effacer = champs.next().and_then(hexa).unwrap_or(0);
            Some((lieu, largeur, poser, effacer))
        })
        .collect();
    m.veille_interdite = !m.compteur_inactivite.is_empty()
        || !m.horodatage_activite.is_empty()
        || !m.drapeau_activite.is_empty();
    if m.veille_interdite {
        println!(
            "  refreshing {} counters, {} timestamps, {} flags every console second",
            m.compteur_inactivite.len(),
            m.horodatage_activite.len(),
            m.drapeau_activite.len()
        );
    }
    m.is_running = true;

    println!("  following the calls until the core reaches {arret:#010x}");
    println!("  giving up after {limite:.0} real seconds\n");

    // The shadow stack. A call is recognised by the link register taking the
    // value of the instruction just after the one that ran, which is what a
    // branch-with-link does and almost nothing else does by accident.
    let mut pile: Vec<Appel> = Vec::new();
    let mut recents: std::collections::VecDeque<Appel> = std::collections::VecDeque::new();
    let debut = std::time::Instant::now();
    let mut atteint = false;

    while debut.elapsed().as_secs_f64() < limite {
        let pc = m.cpu.regs.pc;
        if pc == arret {
            atteint = true;
            break;
        }
        let lr = m.cpu.regs.lr;
        let mots = [
            m.bus.read_u16(pc, &mut m.periph, &m.cpu.nvic),
            m.bus.read_u16(pc.wrapping_add(2), &mut m.periph, &m.cpu.nvic),
        ];
        let longue = matches!(mots[0] & 0xF800, 0xE800 | 0xF000 | 0xF800);
        let suivante = pc.wrapping_add(if longue { 4 } else { 2 });

        if !matches!(m.step(), StepResult::Ok(_)) {
            println!("  the core stopped first");
            break;
        }

        let nouveau_pc = m.cpu.regs.pc;
        let nouveau_lr = m.cpu.regs.lr;
        if nouveau_lr != lr && (nouveau_lr & !1) == suivante && nouveau_pc != suivante {
            let sp = m.cpu.regs.get_sp();
            let cycles = m.cpu.cycles;
            recents.push_back(Appel { depuis: pc, vers: nouveau_pc, retour: suivante, sp, cycles });
            if recents.len() > APPELS {
                recents.pop_front();
            }
            pile.push(Appel { depuis: pc, vers: nouveau_pc, retour: suivante, sp, cycles });
            // A runaway stack means the return detection missed something; keep
            // it bounded rather than eat memory for an hour.
            if pile.len() > 256 {
                pile.remove(0);
            }
        }
        // A frame is left whenever the core arrives at a return address that is
        // still on the shadow stack. Matching only the innermost missed the
        // returns that unwind several frames at once — a POP that loads the
        // program counter, or a tail call — and left the stack growing to
        // hundreds of entries that had long since finished.
        if let Some(k) = pile.iter().rposition(|a| a.retour == nouveau_pc) {
            pile.truncate(k);
        }
    }

    if !atteint {
        println!("  the core did not reach {arret:#010x} in the time allowed.");
        return;
    }

    println!(
        "  reached after {:.1} real seconds, {} console cycles, seconds counter {}",
        debut.elapsed().as_secs_f64(),
        m.cpu.cycles,
        m.periph.snsys.secondes
    );
    println!(
        "  r0 on entry {:#x}, r1 {:#x}, r2 {:#x}, lr {:#010x}\n",
        m.cpu.regs.get_reg(0),
        m.cpu.regs.get_reg(1),
        m.cpu.regs.get_reg(2),
        m.cpu.regs.lr
    );

    println!("  state of the peripherals at this moment:\n");
    println!("    seconds counter      {}", m.periph.snsys.secondes);
    println!(
        "    battery sample       {:#x}  converter {}",
        m.periph.adc_pile.echantillon,
        if m.periph.adc_pile.en_marche { "running" } else { "STOPPED" }
    );
    println!(
        "    PMU ctrl {:#010x}  status {:#010x}",
        m.periph.pmu.ctrl, m.periph.pmu.status
    );
    println!("    SYST_CSR             {:#010x}", m.cpu.nvic.syst_csr);
    // The bytes around the flag that governs this call, whether or not it is
    // being held: if it reads set and the console still winds down, the gate is
    // elsewhere.
    // The outermost gate, and the byte the inner ones read, side by side.
    let externe = m.lire_mot_sram(0x1801_4038);
    println!(
        "\n  outer gate: [0x18014038] = {externe:#010x}, its byte at +20 = {:#04x}",
        m.lire_octet_sram(externe.wrapping_add(20))
    );
    println!("\n  bytes around 0x18000ba0:\n");
    for k in 0..4u32 {
        let base = 0x1800_0b98 + k * 8;
        let mut ligne = format!("    {base:#010x}  ");
        for o in 0..8u32 {
            let v = m.bus.read_u8(base + o, &mut m.periph, &m.cpu.nvic);
            ligne.push_str(&format!("{v:02X} "));
        }
        println!("{ligne}");
    }
    println!();

    println!("  the chain of callers, outermost first:\n");
    println!("  {:<6} {:<12} {:<12} {:<12} {}", "depth", "called from", "entered", "sp", "cycle");
    for (i, a) in pile.iter().enumerate() {
        println!(
            "  {:<6} {:#010x}   {:#010x}   {:#010x}   {}",
            i, a.depuis, a.vers, a.sp, a.cycles
        );
    }
    if pile.is_empty() {
        println!("  (empty — the decision was taken at the top level)");
    }

    println!("\n  what the callers look like, one instruction each:\n");
    for a in pile.iter() {
        let mots = [
            m.bus.read_u16(a.depuis, &mut m.periph, &m.cpu.nvic),
            m.bus.read_u16(a.depuis.wrapping_add(2), &mut m.periph, &m.cpu.nvic),
        ];
        let d = Disassembler::disassemble(a.depuis, &mots);
        println!("  {:#010x}   {} {}", a.depuis, d.mnemonic, d.operands);
    }

    println!("\n  return addresses still on the stack, read upwards from sp:\n");
    let sp = m.cpu.regs.get_sp();
    let mut vus = 0;
    for k in 0..64u32 {
        let adresse = sp.wrapping_add(k * 4);
        let v = m.bus.read_u32(adresse, &mut m.periph, &m.cpu.nvic);
        if code_plausible(v) {
            println!("  {:#010x}   {:#010x}", adresse, v & !1);
            vus += 1;
        }
    }
    if vus == 0 {
        println!("  (none that look like code)");
    }

    println!("\n  the last {} calls, oldest first:\n", recents.len());
    println!("  {:<16} {:<12} {}", "cycle", "from", "to");
    for a in &recents {
        println!("  {:<16} {:#010x}   {:#010x}", a.cycles, a.depuis, a.vers);
    }

    // Ranges asked for by name, read here rather than afterwards. The
    // execute-in-place window is programmable, so a listing taken by a separate
    // tool three seconds after boot shows different instructions from the ones
    // that ran fifty seconds later. Only a reading taken now can be trusted.
    //
    //   CAPYBARA_DESASSEMBLER=0x1005c1ea-0x1005c290,0x10079750-0x100797d0
    for plage in std::env::var("CAPYBARA_DESASSEMBLER")
        .unwrap_or_default()
        .split(',')
        .filter(|p| !p.trim().is_empty())
    {
        let hexa = |v: &str| u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok();
        let Some((a, b)) = plage.split_once('-') else {
            continue;
        };
        let (Some(debut_p), Some(fin_p)) = (hexa(a), hexa(b)) else {
            continue;
        };
        println!("\n  {debut_p:#010x} to {fin_p:#010x}, asked for:\n");
        let mut adresse = debut_p & !1;
        while adresse < fin_p {
            let w1 = m.bus.read_u16(adresse, &mut m.periph, &m.cpu.nvic);
            let w2 = m.bus.read_u16(adresse.wrapping_add(2), &mut m.periph, &m.cpu.nvic);
            let d = Disassembler::disassemble(adresse, &[w1, w2]);
            println!(
                "  {:#010x}   {:04X}{}   {:<10} {}",
                adresse,
                w1,
                if d.is_32bit { format!(" {w2:04X}") } else { "     ".to_string() },
                d.mnemonic,
                d.operands
            );
            adresse += if d.is_32bit { 4 } else { 2 };
        }
    }

    // The caller first: its condition is what we came for. The link register
    // holds the return address, so the call itself is the instruction before.
    let mut sites: Vec<u32> = Vec::new();
    let retour = m.cpu.regs.lr & !1;
    if retour > 4 {
        sites.push(retour.saturating_sub(4));
    }
    if let Some(a) = pile.last() {
        if !sites.contains(&a.depuis) {
            sites.push(a.depuis);
        }
    }
    for a in recents.iter().rev() {
        if !sites.contains(&a.depuis) {
            sites.push(a.depuis);
        }
        if sites.len() >= 6 {
            break;
        }
    }
    for site in sites {
        println!("\n  around {site:#010x}, read with the window as it stands now:\n");
        // Wider before than after: the comparison and the branch that lead to a
        // call sit above it, and that is what has to be read.
        let debut_site = site.saturating_sub(0x60) & !1;
        let mut adresse = debut_site;
        while adresse < site + 0x10 {
            let w1 = m.bus.read_u16(adresse, &mut m.periph, &m.cpu.nvic);
            let w2 = m.bus.read_u16(adresse.wrapping_add(2), &mut m.periph, &m.cpu.nvic);
            let d = Disassembler::disassemble(adresse, &[w1, w2]);
            println!(
                "  {} {:#010x}   {:04X}{}   {:<10} {}",
                if adresse == site { "->" } else { "  " },
                adresse,
                w1,
                if d.is_32bit { format!(" {w2:04X}") } else { "     ".to_string() },
                d.mnemonic,
                d.operands
            );
            adresse += if d.is_32bit { 4 } else { 2 };
        }
    }

    println!(
        "\n  The first listing above is the caller. Read it upwards from the arrow:\n  \
         the branch that reached the call, and the comparison before it. What that\n  \
         comparison reads is the cause of the shutdown."
    );
}
