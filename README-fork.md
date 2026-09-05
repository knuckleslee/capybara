# What this fork changes

A fork of [infinition/capybara](https://github.com/infinition/capybara), a
bare-metal emulator for the Sonix SNC73410 running Tamagotchi Paradise firmware.
Everything upstream does, this does; the changes below are additions and fixes on
top of 0.2.2.

## Speed and smoothness

On a water-edition image the console ran at 0.69× real time and the interface at
about twenty frames per second. It now holds real time with core time to spare,
and the interface follows the display's refresh rate.

| | upstream 0.2.2 | this fork |
|---|---|---|
| console speed | 0.69× | 1.00× |
| interface | ~22 fps | display refresh rate |
| interpreter throughput | 66.1 M cycles/s | unchanged |

The interpreter was not made faster. Two things changed.

**The firmware's waiting is no longer interpreted.** Two thirds of console time
goes into a four-instruction loop waiting for the display's tearing-effect
signal. The core now recognises that kind of loop and advances the clock instead
of unrolling it. Peripherals keep being serviced exactly as before; only the
firmware's observation of them is delayed, by at most 85 microseconds.

**Emulation no longer shares a thread with drawing.** A frame used to cost the
emulation slice plus the drawing, however fast the core ran. A worker thread now
owns the machine and the interface reads a mirror. Periodic disk writes moved off
the emulation loop too, for the same reason.

Both are reversible at runtime without recompiling:

```
CAPYBARA_SANS_REPOS=1     restores the original core
CAPYBARA_UN_SEUL_FIL=1    restores the single-threaded loop
```

## Input fixes

Three defects present in 0.2.2:

- Hovering the mouse over the window discarded all keyboard input, silently.
- The wheel's encoder queue drained slower than keyboard repeat filled it, so
  input piled up and was eventually thrown away in one go.
- `tourner_molette` ignored the magnitude of a turn.

The wheel also no longer borrows the system's keyboard repeat, whose delay is
long and whose rate differs from machine to machine. It follows a curve that
accelerates, which is what makes a dial feel like a dial.

## Three new settings

In the right-click menu, all off by default.

**Keep the console awake.** The firmware counts idle time in a half-word and
compares it against a threshold; when the count wins it sets a bit that makes
the scene machine switch to the shutdown scene, and the screen goes dark. This
clears the count once a second, so it stays an order of magnitude below the
threshold and the firmware never decides it has been idle. Nothing is forced and
no scene is short-circuited: it is what a button press does, without the press.

The address for the water edition, `0x18001BFE`, is built in. Another edition
may count elsewhere, and `inactivite_probe` finds it — look for a half-word that
rises about twenty times a second while idle and returns to zero on a press:

```
CAPYBARA_COMPTEUR_INACTIVITE=0x18001bfe:2
```

`off` switches the mechanism off. A variable set to nothing is treated as unset,
so a stale one left behind in a shell cannot quietly drop the protection.
`CAPYBARA_HORODATAGE_ACTIVITE` and `CAPYBARA_DRAPEAU_ACTIVITE` cover firmware
that records a timestamp or a permission bit instead of counting; both are unset
by default.

The same rule applies to `CAPYBARA_SANS_REPOS` and `CAPYBARA_UN_SEUL_FIL`: they
ask for the old behaviour when set to something, and are ignored when set to
nothing, `0`, `off` or `no`.

**Pause the console when closed.** Saves carry a timestamp and the gap is added
to the seconds counter on reopening, so a Tamagotchi left alone ages, as on the
real device. This makes the counter resume where it stopped.

**Lock on a recovery point.** A padlock per row in the recovery-point list. A
locked point survives pruning and the twelve-hour maximum age until deleted by
hand, and does not count towards interval spacing — otherwise locking one would
delete its neighbour.

## A fixed scene table

Upstream reads the firmware's scene names by their position in the table, which
is one short on the water edition: its first descriptor carries a broken name
pointer, so the search starts at the second and every name shifts by one. Scene
29 showed as `HOME_SPACE` where the firmware calls it `HOME`, scene 117 as
`TAMASPACE_ADDITIONAL` where it is `TAMASPACE_DOWNLOAD`, and so on to the end.

The number is written in the descriptor and is now read from there. The field is
located the same way `table_scenes_probe` locates it, so the probe and the
interface can no longer disagree. The full table, 127 entries with their four
handlers, is in `scenes.md`.

## Six new probes

In the style of the fifty-four already present:

| probe | what it does |
|---|---|
| `repos_probe` | throughput with and without the fast-forward, and the share of time skipped |
| `boucle_probe` | the hottest short loops, disassembled, with what changes between iterations |
| `inactivite_probe` | finds the idle count by its signature: rises while idle, returns to zero on a press |
| `valeur_probe` | watches one place in memory for a whole run and reports every change with the instruction, the registers and the call chain |
| `trace_probe` | records the last sixty thousand instructions before a shutdown |
| `extinction_probe` | follows the calls to a given address and disassembles around them on the spot |

`valeur_probe` is the one to reach for when something in RAM changes and the
question is who changed it; it is what found the idle count.

The window that maps flash at `0x1xxxxxxx` is programmable, so a listing taken by
a separate tool at another moment shows different instructions from the ones that
ran. These probes disassemble from inside the running machine, which is the only
reading that can be trusted; `desassembler` is reliable for PRAM only.

## Notes

`Cargo.toml` is untouched — no new dependencies. Comments on the changed code are
in English; upstream's are in French.

The reasoning behind each change sits in the comments next to it, including the
uncertainties. The idle count at `0x18001BFE` is no longer one of them: the
comparison against its threshold is at `0x00003238`, and the bit it sets is read
by the scene machine at `0x00001F1C`. Both were read from the running machine.

## Licence

GPL-3.0-only, as upstream. Original work by
[infinition](https://github.com/infinition/capybara).
