//! Height-for-width layouts for the details hero, where GtkBox falls short:
//! a horizontal GtkBox works out its minimum height with wrapping text at
//! its narrowest, which left a tall empty gap under the hero.
//!
//! - [`SideBySide`]: the overview and the details table beside it when the
//!   width allows, one above the other when it doesn't.
//! - [`FillLast`]: children in a row at their natural widths, the last one
//!   taking the rest (poster, then the text column).

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use super::scaled;

/// The details column's width, and the gap before it.
const SIDE_WIDTH: i32 = 340;
const GAP: i32 = 32;
/// Narrower than this, the details go under the overview instead.
const SIDE_BY_SIDE_FROM: i32 = 820;
/// Between the two when stacked: the hero's own spacing.
const STACK_GAP: i32 = 8;

/// Between [`FillLast`]'s children.
const ROW_SPACING: i32 = 24;

glib::wrapper! {
    pub struct SideBySide(ObjectSubclass<imp::SideBySide>) @extends gtk::LayoutManager;
}

impl SideBySide {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

glib::wrapper! {
    pub struct FillLast(ObjectSubclass<imp::FillLast>) @extends gtk::LayoutManager;
}

impl FillLast {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

/// [`FillLast`]'s column widths at `width`: the others' natural widths, the
/// last child the rest (never under its minimum).
fn fill_last_widths(width: i32, children: &[gtk::Widget]) -> Vec<i32> {
    let Some((last, others)) = children.split_last() else {
        return Vec::new();
    };
    let mut widths: Vec<i32> = others
        .iter()
        .map(|child| child.measure(gtk::Orientation::Horizontal, -1).1)
        .collect();
    let used: i32 = widths.iter().sum::<i32>() + ROW_SPACING * others.len() as i32;
    let last_min = last.measure(gtk::Orientation::Horizontal, -1).0;
    widths.push((width - used).max(last_min));
    widths
}

fn translate(x: i32, y: i32) -> Option<gsk::Transform> {
    Some(gsk::Transform::new().translate(&graphene::Point::new(x as f32, y as f32)))
}

/// The children's widths when side by side at `width`, else `None`.
fn split(width: i32, visible: usize) -> Option<(i32, i32)> {
    let side = scaled(SIDE_WIDTH);
    (visible == 2 && width >= scaled(SIDE_BY_SIDE_FROM)).then(|| (width - scaled(GAP) - side, side))
}

fn visible_children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut children = Vec::new();
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.is_visible() {
            children.push(current.clone());
        }
        child = current.next_sibling();
    }
    children
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SideBySide;

    #[glib::object_subclass]
    impl ObjectSubclass for SideBySide {
        const NAME: &'static str = "EcpSideBySide";
        type Type = super::SideBySide;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for SideBySide {}

    impl LayoutManagerImpl for SideBySide {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let children = visible_children(widget);
            if children.is_empty() {
                return (0, 0, -1, -1);
            }
            if orientation == gtk::Orientation::Horizontal {
                let sizes: Vec<(i32, i32)> = children
                    .iter()
                    .map(|child| {
                        let (min, nat, _, _) = child.measure(orientation, -1);
                        (min, nat)
                    })
                    .collect();
                let min = sizes.iter().map(|size| size.0).max().unwrap_or(0);
                let mut nat = sizes[0].1;
                if children.len() == 2 {
                    nat += scaled(GAP) + scaled(SIDE_WIDTH);
                }
                return (min, nat.max(min), -1, -1);
            }
            if for_size >= 0
                && let Some((main, side)) = split(for_size, children.len())
            {
                let (a, b) = (
                    children[0].measure(orientation, main),
                    children[1].measure(orientation, side),
                );
                return (a.0.max(b.0), a.1.max(b.1), -1, -1);
            }
            let gaps = STACK_GAP * (children.len() as i32 - 1);
            let (mut min, mut nat) = (gaps, gaps);
            for child in &children {
                let (child_min, child_nat, _, _) = child.measure(orientation, for_size);
                min += child_min;
                nat += child_nat;
            }
            (min, nat, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let children = visible_children(widget);
            if let Some((main, side)) = split(width, children.len()) {
                children[0].allocate(main, height, -1, None);
                children[1].allocate(side, height, -1, translate(main + scaled(GAP), 0));
                return;
            }
            let mut y = 0;
            for child in &children {
                let (_, child_height, _, _) = child.measure(gtk::Orientation::Vertical, width);
                child.allocate(width, child_height, -1, translate(0, y));
                y += child_height + STACK_GAP;
            }
        }
    }

    #[derive(Default)]
    pub struct FillLast;

    #[glib::object_subclass]
    impl ObjectSubclass for FillLast {
        const NAME: &'static str = "EcpFillLast";
        type Type = super::FillLast;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for FillLast {}

    impl LayoutManagerImpl for FillLast {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let children = visible_children(widget);
            let Some((last, others)) = children.split_last() else {
                return (0, 0, -1, -1);
            };
            if orientation == gtk::Orientation::Horizontal {
                let fixed: i32 = others
                    .iter()
                    .map(|child| child.measure(orientation, -1).1)
                    .sum::<i32>()
                    + ROW_SPACING * others.len() as i32;
                let (min, nat, _, _) = last.measure(orientation, -1);
                return (fixed + min, fixed + nat, -1, -1);
            }
            // Each child's height at the width it will actually get, so the
            // minimum is that too (GtkBox used the narrowest text column).
            let height = if for_size < 0 {
                children
                    .iter()
                    .map(|child| child.measure(orientation, -1).1)
                    .max()
            } else {
                children
                    .iter()
                    .zip(fill_last_widths(for_size, &children))
                    .map(|(child, width)| child.measure(orientation, width).1)
                    .max()
            }
            .unwrap_or(0);
            (height, height, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let children = visible_children(widget);
            let mut x = 0;
            for (child, child_width) in children.iter().zip(fill_last_widths(width, &children)) {
                child.allocate(child_width, height, -1, translate(x, 0));
                x += child_width + ROW_SPACING;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_only_with_room_for_both() {
        // UI scale 1.0 in tests.
        assert_eq!(split(1200, 2), Some((1200 - GAP - SIDE_WIDTH, SIDE_WIDTH)));
        assert_eq!(split(SIDE_BY_SIDE_FROM - 1, 2), None);
        assert_eq!(split(1200, 1), None);
    }
}
