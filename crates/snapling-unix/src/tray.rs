//! Tray icon through StatusNotifierItem: each menu row runs `snapling` with its flag, the way
//! compositor key bindings do.

use ksni::blocking::TrayMethods;
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Tray};

/// A menu row: title and the flag to run with; `None` is a separator.
pub type Row = Option<(String, &'static str)>;

struct Snapling {
    rows: Vec<Row>,
    icon: Icon,
}

impl Tray for Snapling {
    fn id(&self) -> String {
        "snapling".into()
    }

    fn title(&self) -> String {
        "Snapling".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![Icon {
            width: self.icon.width,
            height: self.icon.height,
            data: self.icon.data.clone(),
        }]
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.rows
            .iter()
            .map(|row| match row {
                None => MenuItem::Separator,
                Some((label, flag)) => {
                    let flag = *flag;
                    StandardItem {
                        label: label.clone(),
                        activate: Box::new(move |_: &mut Self| run(flag)),
                        ..Default::default()
                    }
                    .into()
                }
            })
            .collect()
    }
}

fn run(flag: &str) {
    if flag == "--quit" {
        std::process::exit(0);
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg(flag).spawn();
    }
}

/// Shows the tray and never returns. `argb`: the icon, 32×32, alpha first.
pub fn run_forever(rows: Vec<Row>, argb: Vec<u8>) -> Result<(), String> {
    let icon = Icon {
        width: 32,
        height: 32,
        data: argb,
    };
    let handle = Snapling { rows, icon }.spawn().map_err(|e| e.to_string())?;
    loop {
        std::thread::park();
        if handle.is_closed() {
            return Ok(());
        }
    }
}
