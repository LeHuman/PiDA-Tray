//! Pure-vector icons drawn with the egui `Painter` (no image assets).

use std::f32::consts::{PI, TAU};

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::catalog::IconKind;

fn arc(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let steps = 24;
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}

pub fn draw_kind(p: &Painter, kind: IconKind, rect: Rect, color: Color32) {
    match kind {
        IconKind::Light => light(p, rect, color),
        IconKind::Fan => fan(p, rect, color),
        IconKind::Display => display(p, rect, color),
        IconKind::Computer => computer(p, rect, color),
        IconKind::Audio => audio(p, rect, color),
        IconKind::Sensor => sensor(p, rect, color),
        IconKind::Camera => camera(p, rect, color),
        IconKind::Lock => lock(p, rect, color),
        IconKind::Cover => cover(p, rect, color),
        IconKind::Power => power(p, rect, color),
        IconKind::Generic => generic(p, rect, color),
    }
}

fn stroke(rect: Rect, color: Color32) -> Stroke {
    Stroke::new((rect.width() * 0.09).max(1.2), color)
}

pub fn power(p: &Painter, rect: Rect, color: Color32) {
    let (c, r, s) = (rect.center(), rect.width() * 0.32, stroke(rect, color));
    // Open ring with a gap at the top, plus a vertical bar.
    p.add(Shape::line(
        arc(c, r, -PI / 2.0 + 0.6, -PI / 2.0 + TAU - 0.6),
        s,
    ));
    p.line_segment([c - vec2(0.0, r + 1.0), c - vec2(0.0, 1.0)], s);
}

pub fn light(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let r = rect.width() * 0.26;
    p.circle_stroke(c - vec2(0.0, rect.height() * 0.08), r, s);
    for dy in [0.30, 0.40] {
        let y = c.y + rect.height() * dy - rect.height() * 0.02;
        p.line_segment([pos2(c.x - r * 0.6, y), pos2(c.x + r * 0.6, y)], s);
    }
}

pub fn fan(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let r = rect.width() * 0.40;
    p.circle_stroke(c, r, s);
    p.circle_filled(c, rect.width() * 0.06, color);
    for i in 0..3 {
        let a = i as f32 * TAU / 3.0 - PI / 2.0;
        let tip = c + vec2(a.cos(), a.sin()) * r * 0.85;
        let ctrl = c + vec2((a + 0.7).cos(), (a + 0.7).sin()) * r * 0.55;
        p.add(Shape::line(vec![c, ctrl, tip], s));
    }
}

pub fn display(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let screen = Rect::from_center_size(
        c - vec2(0.0, rect.height() * 0.06),
        vec2(rect.width() * 0.78, rect.height() * 0.50),
    );
    p.rect_stroke(screen, 2.0, s, StrokeKind::Middle);
    let base_y = screen.bottom() + rect.height() * 0.14;
    p.line_segment([pos2(c.x, screen.bottom()), pos2(c.x, base_y)], s);
    p.line_segment(
        [
            pos2(c.x - rect.width() * 0.2, base_y),
            pos2(c.x + rect.width() * 0.2, base_y),
        ],
        s,
    );
}

pub fn computer(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let tower = Rect::from_center_size(c, vec2(rect.width() * 0.46, rect.height() * 0.80));
    p.rect_stroke(tower, 2.0, s, StrokeKind::Middle);
    p.line_segment(
        [
            pos2(tower.left() + 4.0, tower.top() + rect.height() * 0.16),
            pos2(tower.right() - 4.0, tower.top() + rect.height() * 0.16),
        ],
        s,
    );
    p.circle_filled(
        pos2(c.x, tower.bottom() - rect.height() * 0.14),
        rect.width() * 0.04,
        color,
    );
}

pub fn audio(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let body = Rect::from_center_size(c, vec2(rect.width() * 0.48, rect.height() * 0.78));
    p.rect_stroke(body, 3.0, s, StrokeKind::Middle);
    p.circle_stroke(
        pos2(c.x, body.top() + rect.height() * 0.20),
        rect.width() * 0.10,
        s,
    );
    p.circle_stroke(
        pos2(c.x, body.bottom() - rect.height() * 0.17),
        rect.width() * 0.15,
        s,
    );
}

pub fn sensor(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let dot = c - vec2(rect.width() * 0.15, 0.0);
    p.circle_filled(dot, rect.width() * 0.07, color);
    for r in [0.22, 0.36] {
        p.add(Shape::line(
            arc(dot, rect.width() * r, -PI / 4.0, PI / 4.0),
            s,
        ));
    }
}

pub fn camera(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let body = Rect::from_center_size(c, vec2(rect.width() * 0.78, rect.height() * 0.52));
    p.rect_stroke(body, 3.0, s, StrokeKind::Middle);
    p.circle_stroke(c, rect.width() * 0.15, s);
    p.circle_filled(
        pos2(
            body.right() - rect.width() * 0.10,
            body.top() + rect.height() * 0.10,
        ),
        rect.width() * 0.03,
        color,
    );
}

pub fn lock(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let body = Rect::from_center_size(
        c + vec2(0.0, rect.height() * 0.10),
        vec2(rect.width() * 0.60, rect.height() * 0.46),
    );
    let shackle_center = pos2(c.x, body.top());
    p.add(Shape::line(
        arc(shackle_center, rect.width() * 0.22, 0.0, PI),
        s,
    ));
    p.rect_stroke(body, 3.0, s, StrokeKind::Middle);
    p.circle_filled(
        pos2(c.x, body.center().y - rect.height() * 0.02),
        rect.width() * 0.045,
        color,
    );
    p.line_segment(
        [
            pos2(c.x, body.center().y),
            pos2(c.x, body.center().y + rect.height() * 0.10),
        ],
        s,
    );
}

pub fn cover(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let frame = Rect::from_center_size(c, vec2(rect.width() * 0.70, rect.height() * 0.66));
    p.rect_stroke(frame, 2.0, s, StrokeKind::Middle);
    for i in 1..=3 {
        let y = frame.top() + frame.height() * i as f32 / 4.0;
        p.line_segment([pos2(frame.left(), y), pos2(frame.right(), y)], s);
    }
    let cord_x = frame.right() + rect.width() * 0.08;
    p.line_segment(
        [
            pos2(cord_x, frame.top()),
            pos2(cord_x, frame.bottom() + rect.height() * 0.06),
        ],
        s,
    );
}

pub fn generic(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let body = Rect::from_center_size(c, vec2(rect.width() * 0.52, rect.height() * 0.52));
    p.rect_stroke(body, 4.0, s, StrokeKind::Middle);
    p.circle_filled(c, rect.width() * 0.08, color);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        let dir = vec2(dx, dy);
        let edge = if dx != 0.0 {
            body.width() * 0.5
        } else {
            body.height() * 0.5
        };
        p.line_segment([c + dir * edge, c + dir * (edge + rect.width() * 0.10)], s);
    }
}

pub fn refresh(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let r = rect.width() * 0.30;
    let (a0, a1) = (-PI * 0.25, PI * 1.35);
    p.add(Shape::line(arc(c, r, a0, a1), s));
    let end = c + vec2(a1.cos(), a1.sin()) * r;
    let dir = vec2(-a1.sin(), a1.cos()); // tangent at the arc end
    let n = vec2(-dir.y, dir.x);
    p.add(Shape::line(
        vec![
            end + dir * -r * 0.1 + n * r * 0.45,
            end + dir * r * 0.1,
            end + dir * -r * 0.1 - n * r * 0.45,
        ],
        s,
    ));
}

pub fn gear(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let r = rect.width() * 0.22;
    p.circle_stroke(c, r, s);
    for i in 0..8 {
        let a = i as f32 * TAU / 8.0;
        let d = vec2(a.cos(), a.sin());
        p.line_segment([c + d * (r + 1.5), c + d * (r + rect.width() * 0.12)], s);
    }
}

pub fn pin(p: &Painter, rect: Rect, color: Color32, filled: bool) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let head = Rect::from_center_size(
        c - vec2(0.0, rect.height() * 0.10),
        vec2(rect.width() * 0.30, rect.height() * 0.34),
    );
    if filled {
        p.rect_filled(head, 2.0, color);
    } else {
        p.rect_stroke(head, 2.0, s, StrokeKind::Middle);
    }
    let y = head.bottom();
    p.line_segment(
        [
            pos2(c.x - rect.width() * 0.22, y),
            pos2(c.x + rect.width() * 0.22, y),
        ],
        s,
    );
    p.line_segment([pos2(c.x, y), pos2(c.x, y + rect.height() * 0.26)], s);
}

pub fn close(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let d = rect.width() * 0.20;
    p.line_segment([c - Vec2::splat(d), c + Vec2::splat(d)], s);
    p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], s);
}

pub fn back(p: &Painter, rect: Rect, color: Color32) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let d = rect.width() * 0.18;
    p.add(Shape::line(
        vec![
            c + vec2(d * 0.6, -d * 1.2),
            c + vec2(-d * 0.6, 0.0),
            c + vec2(d * 0.6, d * 1.2),
        ],
        s,
    ));
}

/// Disclosure chevron: points down when expanded, up when collapsed.
pub fn chevron(p: &Painter, rect: Rect, color: Color32, expanded: bool) {
    let (c, s) = (rect.center(), stroke(rect, color));
    let d = rect.width() * 0.20;
    let (y_outer, y_tip) = if expanded {
        (d * 0.5, -d * 0.45)
    } else {
        (-d * 0.5, d * 0.45)
    };
    p.add(Shape::line(
        vec![
            c + vec2(-d, y_outer),
            c + vec2(0.0, y_tip),
            c + vec2(d, y_outer),
        ],
        s,
    ));
}
