use tauri::{
    App,
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};

pub const OPEN_MENU_ID: &str = "open";
pub const SETTINGS_MENU_ID: &str = "settings";
pub const QUIT_MENU_ID: &str = "quit";

pub fn install(app: &mut App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, OPEN_MENU_ID, "Open DeskAct", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, SETTINGS_MENU_ID, "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_MENU_ID, "Quit DeskAct", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &settings, &quit])?;

    TrayIconBuilder::with_id("deskact-tray")
        .icon(tray_image())
        .tooltip("DeskAct")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            OPEN_MENU_ID => super::window_behavior::open_composer(app),
            SETTINGS_MENU_ID => super::window_behavior::open_settings(app),
            QUIT_MENU_ID => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn tray_image() -> Image<'static> {
    let mut rgba = vec![0; 16 * 16 * 4];
    for y in 2..14 {
        for x in 2..14 {
            let edge = (x == 2 || x == 13) && !(4..=11).contains(&y)
                || (y == 2 || y == 13) && !(4..=11).contains(&x);
            if !edge {
                set_pixel(&mut rgba, x, y, [38, 44, 56, 255]);
            }
        }
    }
    for y in 4..12 {
        for x in 5..11 {
            if x == 5 || (x == 10 && (5..=10).contains(&y)) || (y == 4 || y == 11) && x < 10 {
                set_pixel(&mut rgba, x, y, [232, 237, 245, 255]);
            }
        }
    }
    Image::new_owned(rgba, 16, 16)
}

fn set_pixel(image: &mut [u8], x: usize, y: usize, color: [u8; 4]) {
    let offset = (y * 16 + x) * 4;
    image[offset..offset + 4].copy_from_slice(&color);
}
