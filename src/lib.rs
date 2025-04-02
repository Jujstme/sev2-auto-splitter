#![no_std]
#![warn(
    clippy::complexity,
    clippy::correctness,
    clippy::perf,
    clippy::style,
    clippy::undocumented_unsafe_blocks,
    rust_2018_idioms
)]

use asr::{
    Address, Process,
    file_format::pe,
    future::{next_tick, retry},
    settings::Gui,
    string::ArrayCString,
    time::Duration,
    timer::{self, TimerState},
    watcher::Watcher,
};

asr::async_main!(stable);
asr::panic_handler!();

const PROCESS_NAMES: &[&str] = &["SniperEliteV2.exe", "SEV2_Remastered.exe"];

async fn main() {
    let mut settings = Settings::register();

    loop {
        // Hook to the target process
        let (process_name, process) = retry(|| {
            PROCESS_NAMES
                .iter()
                .find_map(|&name| Some((name, Process::attach(name)?)))
        })
        .await;

        process
            .until_closes(async {
                // Once the target has been found and attached to, set up some default watchers
                let mut watchers = Watchers::default();

                // Perform memory scanning to look for the addresses we need
                let addresses = Memory::init(&process, process_name).await;

                loop {
                    // Splitting logic. Adapted from OG LiveSplit:
                    // Order of execution
                    // 1. update() will always be run first. There are no conditions on the execution of this action.
                    // 2. If the timer is currently either running or paused, then the isLoading, gameTime, and reset actions will be run.
                    // 3. If reset does not return true, then the split action will be run.
                    // 4. If the timer is currently not running (and not paused), then the start action will be run.
                    settings.update();

                    if watchers.slow_pc_mode != settings.slow_pc_mode {
                        asr::set_tick_rate(match settings.slow_pc_mode {
                            true => 60.0,
                            false => 120.0,
                        });

                        watchers.slow_pc_mode = settings.slow_pc_mode;
                    }

                    update_loop(&process, &addresses, &mut watchers);

                    if [TimerState::Running, TimerState::Paused].contains(&timer::state()) {
                        match is_loading(&watchers, &settings) {
                            Some(true) => timer::pause_game_time(),
                            Some(false) => timer::resume_game_time(),
                            _ => (),
                        }

                        match game_time(&watchers, &settings, &addresses) {
                            Some(x) => timer::set_game_time(x),
                            _ => (),
                        }

                        match reset(&watchers, &settings) {
                            true => timer::reset(),
                            _ => match split(&watchers, &settings) {
                                true => timer::split(),
                                _ => (),
                            },
                        }
                    }

                    if timer::state().eq(&TimerState::NotRunning) && start(&watchers, &settings) {
                        timer::start();
                        timer::pause_game_time();

                        match is_loading(&watchers, &settings) {
                            Some(true) => timer::pause_game_time(),
                            Some(false) => timer::resume_game_time(),
                            _ => (),
                        }
                    }

                    next_tick().await;
                }
            })
            .await;
    }
}

#[derive(Gui)]
struct Settings {
    /// IL mode
    #[default = false]
    individual_level: bool,
    /// Slow PC mode (reduces the refresh rate from 120hz to 60hz)
    #[default = false]
    slow_pc_mode: bool,
}

struct Memory {
    start: Address,
    load: Address,
    splash: Address,
    level: Address,
    bullet: Address,
    objective: Address,
    mc: Address,
}

impl Memory {
    async fn init(process: &Process, main_module_name: &str) -> Self {
        let main_module_base = retry(|| process.get_module_address(main_module_name)).await;
        let main_module_size = retry(|| pe::read_size_of_image(process, main_module_base)).await;

        // let is_64_bit = retry(|| MachineType::pointer_size(MachineType::read(process, main_module_base)?)).await == PointerSize::Bit64;

        match main_module_size {
            0x1154000 => Self {
                // remastered
                start: main_module_base + 0x799A77,
                load: main_module_base + 0x774FE3,
                splash: main_module_base + 0x74C670,
                level: main_module_base + 0x7CFC7D,
                bullet: main_module_base + 0x76DD17,
                objective: main_module_base + 0x7CF568,
                mc: main_module_base + 0x799A63,
            },
            _ => Self {
                // OG?
                start: main_module_base + 0x689FE2,
                load: main_module_base + 0x67FC38,
                splash: main_module_base + 0x653B40,
                level: main_module_base + 0x685F31,
                bullet: main_module_base + 0x65B917,
                objective: main_module_base + 0x656F3C,
                mc: main_module_base + 0x689FD2,
            },
        }
    }
}

#[derive(Default)]
struct Watchers {
    slow_pc_mode: bool,
    start_byte: Watcher<u8>,
    load_byte: Watcher<u8>,
    splash_byte: Watcher<u8>,
    level: Watcher<ArrayCString<2>>,
    bullet_cam: Watcher<u8>,
    objective: Watcher<u8>,
    mc: Watcher<u8>,
}

fn update_loop(process: &Process, memory: &Memory, watchers: &mut Watchers) {
    watchers
        .start_byte
        .update_infallible(process.read(memory.start).unwrap_or_default());

    watchers
        .load_byte
        .update_infallible(process.read(memory.load).unwrap_or_else(|_| 1));
    watchers
        .splash_byte
        .update_infallible(process.read(memory.splash).unwrap_or_else(|_| 1));

    watchers
        .bullet_cam
        .update_infallible(process.read(memory.bullet).unwrap_or_default());
    watchers
        .objective
        .update_infallible(process.read(memory.objective).unwrap_or_default());
    watchers
        .mc
        .update_infallible(process.read(memory.mc).unwrap_or_default());

    watchers
        .level
        .update_infallible(process.read(memory.level).unwrap_or_default());
}

fn start(watchers: &Watchers, settings: &Settings) -> bool {
    match settings.individual_level {
        true => watchers.splash_byte.pair.is_some_and(|val| {
            val.changed_from_to(&0, &1)
                && watchers
                    .level
                    .pair
                    .is_some_and(|val| !val.current.matches("nu"))
        }),
        false => watchers
            .start_byte
            .pair
            .is_some_and(|val| val.changed_to(&1)),
    }
}

fn is_loading(watchers: &Watchers, _settings: &Settings) -> Option<bool> {
    Some(watchers.load_byte.pair?.current == 1 && watchers.splash_byte.pair?.current == 1)
}

fn split(watchers: &Watchers, settings: &Settings) -> bool {
    match settings.individual_level {
        true => watchers.mc.pair.is_some_and(|val| val.changed_to(&1)),
        false => {
            watchers.level.pair.is_some_and(|val| {
                val.changed()
                    && !val.current.is_empty()
                    && !val.current.matches("nu")
                    && !val.matches("Tu")
            }) || (watchers
                .level
                .pair
                .is_some_and(|val| val.current.matches("Br"))
                && watchers.bullet_cam.pair.is_some_and(|val| val.current == 1)
                && watchers.objective.pair.is_some_and(|val| val.current == 3))
        }
    }
}

fn game_time(_watchers: &Watchers, _settings: &Settings, _addresses: &Memory) -> Option<Duration> {
    None
}

fn reset(_watchers: &Watchers, _settings: &Settings) -> bool {
    false
}
