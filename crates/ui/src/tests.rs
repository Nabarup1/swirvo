use super::*;
use crate::widgets::*;

fn ui() -> Option<Ui> {
    Fonts::system().map(|f| Ui::new(Theme::dark(), f))
}

fn frame(ui: &mut Ui, f: impl FnOnce(&mut Ui)) {
    ui.input.dt = 1.0 / 60.0;
    ui.input.time += 1.0 / 60.0;
    ui.begin_frame(400, 300, 1.0);
    f(ui);
    ui.end_frame();
}

#[test]
fn button_clicks_on_release_inside() {
    let Some(mut ui) = ui() else { return };
    let r = Rect::new(10.0, 10.0, 100.0, 32.0);
    let id = Id::new("b");
    let clicks = std::cell::Cell::new(0);
    let run = |ui: &mut Ui| {
        if button(ui, id, r, "Go", None, ButtonKind::Primary, true).clicked {
            clicks.set(clicks.get() + 1);
        }
    };
    ui.input.pointer = Some(Point::new(50.0, 20.0));
    frame(&mut ui, run); // hover
    ui.input.pressed = true;
    ui.input.down = true;
    frame(&mut ui, run);
    ui.input.down = false;
    ui.input.released = true;
    frame(&mut ui, run);
    assert_eq!(clicks.get(), 1);
    // Press inside, release outside: no click.
    ui.input.pressed = true;
    ui.input.down = true;
    frame(&mut ui, run);
    ui.input.pointer = Some(Point::new(300.0, 200.0));
    ui.input.down = false;
    ui.input.released = true;
    frame(&mut ui, run);
    assert_eq!(clicks.get(), 1);
}

#[test]
fn topmost_widget_wins_hover() {
    let Some(mut ui) = ui() else { return };
    let outer = Rect::new(0.0, 0.0, 200.0, 200.0);
    let inner = Rect::new(50.0, 50.0, 40.0, 40.0);
    ui.input.pointer = Some(Point::new(60.0, 60.0));
    let mut hovered = (false, false);
    for _ in 0..3 {
        frame(&mut ui, |ui| {
            let a = ui.interact(Id::new("outer"), outer).hovered;
            let b = ui.interact(Id::new("inner"), inner).hovered;
            hovered = (a, b);
        });
    }
    assert_eq!(hovered, (false, true));
}

#[test]
fn text_field_edits() {
    let Some(mut ui) = ui() else { return };
    let r = Rect::new(10.0, 10.0, 200.0, 32.0);
    let id = Id::new("t");
    let mut s = String::from("1-3");
    ui.input.pointer = Some(Point::new(150.0, 20.0));
    frame(&mut ui, |ui| {
        text_field(ui, id, r, &mut s, "", false);
    });
    ui.input.pressed = true;
    ui.input.down = true;
    frame(&mut ui, |ui| {
        text_field(ui, id, r, &mut s, "", false);
    });
    ui.input.down = false;
    ui.input.released = true;
    ui.input.text.push_str(", 7");
    frame(&mut ui, |ui| {
        text_field(ui, id, r, &mut s, "", false);
    });
    assert_eq!(s, "1-3, 7");
    ui.input.keys.push((Key::Backspace, Mods::default()));
    let mut submitted = false;
    ui.input.keys.push((Key::Enter, Mods::default()));
    frame(&mut ui, |ui| {
        submitted = text_field(ui, id, r, &mut s, "", false).submitted;
    });
    assert_eq!(s, "1-3, ");
    assert!(submitted);
}

#[test]
fn dropdown_picks_from_popup() {
    let Some(mut ui) = ui() else { return };
    let r = Rect::new(10.0, 10.0, 160.0, 32.0);
    let id = Id::new("d");
    let items: Vec<String> = ["JPG", "PNG", "WebP"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut sel = 0;
    let click = |ui: &mut Ui, p: Point, sel: &mut usize| {
        ui.input.pointer = Some(p);
        frame(ui, |ui| {
            dropdown(ui, id, r, &items, sel);
        });
        ui.input.pressed = true;
        ui.input.down = true;
        frame(ui, |ui| {
            dropdown(ui, id, r, &items, sel);
        });
        ui.input.down = false;
        ui.input.released = true;
        frame(ui, |ui| {
            dropdown(ui, id, r, &items, sel);
        });
    };
    click(&mut ui, Point::new(50.0, 25.0), &mut sel);
    // The popup opens below: row 2 ("WebP") is at y ≈ 46 + 4 + 2·30.
    click(
        &mut ui,
        Point::new(50.0, 46.0 + 4.0 + 2.0 * 30.0 + 15.0),
        &mut sel,
    );
    frame(&mut ui, |ui| {
        dropdown(ui, id, r, &items, &mut sel);
    });
    assert_eq!(sel, 2);
}

#[test]
fn draws_text_and_shapes() {
    let Some(mut ui) = ui() else { return };
    ui.theme = Theme::light();
    frame(&mut ui, |ui| {
        ui.glass(
            Rect::new(20.0, 20.0, 200.0, 100.0),
            10.0,
            ui.theme.card,
            12.0,
        );
        ui.text(
            30.0,
            60.0,
            "Hello Swirvo",
            20.0,
            Weight::Semibold,
            Rgba::BLACK,
        );
    });
    let pm = ui.pixmap();
    // Some text pixels are dark and opaque.
    let dark = pm
        .pixels()
        .iter()
        .filter(|p| p.alpha() > 200 && p.red() < 80)
        .count();
    assert!(dark > 100, "{dark}");
    // Ellipsis keeps the extension.
    let s = ui.fonts.ellipsize(
        "Quarterly report final v2.pdf",
        13.0,
        Weight::Regular,
        110.0,
    );
    assert!(s.ends_with("….pdf"), "{s}");
    let lines = ui
        .fonts
        .wrap("one two three four five six", 13.0, Weight::Regular, 60.0);
    assert!(lines.len() >= 3);
}
