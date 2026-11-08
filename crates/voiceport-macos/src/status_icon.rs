//! Original bear-and-microphone menu-bar mark for VoicePort.
//! Round ears identify the bear; a microphone replaces the muzzle.
use block2::RcBlock;
use objc2::{rc::Retained, runtime::Bool};
use objc2_app_kit::{NSBezierPath, NSColor, NSImage, NSWindingRule};
use objc2_foundation::{NSPoint, NSRect, NSSize};

const SIZE: f64 = 18.;
fn point(x: f64, y: f64) -> NSPoint {
    NSPoint::new(x * SIZE / 24., y * SIZE / 24.)
}
fn curve(path: &NSBezierPath, c1: (f64, f64), c2: (f64, f64), end: (f64, f64)) {
    path.curveToPoint_controlPoint1_controlPoint2(
        point(end.0, end.1),
        point(c1.0, c1.1),
        point(c2.0, c2.1),
    );
}

pub(super) fn image() -> Retained<NSImage> {
    let drawing = RcBlock::new(|_rect: NSRect| {
        NSColor::blackColor().setFill();
        let path = NSBezierPath::new();
        // Cut the eyes and microphone out of the silhouette. Transparent holes
        // preserve contrast when AppKit tints the template on either appearance.
        path.setWindingRule(NSWindingRule::EvenOdd);
        path.moveToPoint(point(4.1, 8.3));
        curve(&path, (1.4, 7.1), (1.2, 3.3), (3.7, 2.1));
        curve(&path, (6.0, 0.9), (8.8, 2.1), (9.2, 4.5));
        curve(&path, (11.1, 4.0), (12.9, 4.0), (14.8, 4.5));
        curve(&path, (15.2, 2.1), (18.0, 0.9), (20.3, 2.1));
        curve(&path, (22.8, 3.3), (22.6, 7.1), (19.9, 8.3));
        curve(&path, (21.5, 10.0), (22.0, 12.0), (22.0, 14.2));
        curve(&path, (22.0, 19.9), (17.8, 23.0), (12.0, 23.0));
        curve(&path, (6.2, 23.0), (2.0, 19.9), (2.0, 14.2));
        curve(&path, (2.0, 12.0), (2.5, 10.0), (4.1, 8.3));
        path.closePath();
        path.moveToPoint(point(8.0, 8.9));
        path.lineToPoint(point(8.0, 8.9));
        curve(&path, (8.497056275, 8.9), (8.9, 9.302943725), (8.9, 9.8));
        path.lineToPoint(point(8.9, 9.8));
        curve(
            &path,
            (8.9, 10.297056275000001),
            (8.497056275, 10.700000000000001),
            (8.0, 10.700000000000001),
        );
        path.lineToPoint(point(8.0, 10.700000000000001));
        curve(
            &path,
            (7.502943725, 10.700000000000001),
            (7.1, 10.297056275000001),
            (7.1, 9.8),
        );
        path.lineToPoint(point(7.1, 9.8));
        curve(&path, (7.1, 9.302943725), (7.502943725, 8.9), (8.0, 8.9));
        path.closePath();
        path.moveToPoint(point(16.0, 8.9));
        path.lineToPoint(point(15.999999999999998, 8.9));
        curve(&path, (16.497056275, 8.9), (16.9, 9.302943725), (16.9, 9.8));
        path.lineToPoint(point(16.9, 9.8));
        curve(
            &path,
            (16.9, 10.297056275000001),
            (16.497056275, 10.700000000000001),
            (15.999999999999998, 10.700000000000001),
        );
        path.lineToPoint(point(16.0, 10.700000000000001));
        curve(
            &path,
            (15.502943725, 10.700000000000001),
            (15.1, 10.297056275000001),
            (15.1, 9.8),
        );
        path.lineToPoint(point(15.1, 9.8));
        curve(&path, (15.1, 9.302943725), (15.502943725, 8.9), (16.0, 8.9));
        path.closePath();
        path.moveToPoint(point(12.0, 10.5));
        path.lineToPoint(point(11.999999999999998, 10.5));
        curve(
            &path,
            (12.994112549999999, 10.5),
            (13.799999999999999, 11.30588745),
            (13.799999999999999, 12.3),
        );
        path.lineToPoint(point(13.799999999999999, 14.1));
        curve(
            &path,
            (13.799999999999999, 15.09411255),
            (12.994112549999999, 15.9),
            (11.999999999999998, 15.9),
        );
        path.lineToPoint(point(12.0, 15.9));
        curve(
            &path,
            (11.00588745, 15.9),
            (10.2, 15.09411255),
            (10.2, 14.1),
        );
        path.lineToPoint(point(10.2, 12.3));
        curve(
            &path,
            (10.2, 11.30588745),
            (11.00588745, 10.5),
            (12.0, 10.5),
        );
        path.closePath();
        path.moveToPoint(point(8.1, 13.3));
        curve(&path, (8.1, 12.9), (8.4, 12.6), (8.8, 12.6));
        curve(&path, (9.2, 12.6), (9.5, 12.9), (9.5, 13.3));
        path.lineToPoint(point(9.5, 14.8));
        curve(&path, (9.5, 16.4), (10.5, 17.5), (12.0, 17.5));
        curve(&path, (13.5, 17.5), (14.5, 16.4), (14.5, 14.8));
        path.lineToPoint(point(14.5, 13.3));
        curve(&path, (14.5, 12.9), (14.8, 12.6), (15.2, 12.6));
        curve(&path, (15.6, 12.6), (15.9, 12.9), (15.9, 13.3));
        path.lineToPoint(point(15.9, 14.8));
        curve(&path, (15.9, 16.9), (14.6, 18.5), (12.7, 18.9));
        path.lineToPoint(point(12.7, 20.1));
        path.lineToPoint(point(13.5, 20.1));
        curve(&path, (13.9, 20.1), (14.2, 20.4), (14.2, 20.8));
        curve(&path, (14.2, 21.2), (13.9, 21.5), (13.5, 21.5));
        path.lineToPoint(point(10.5, 21.5));
        curve(&path, (10.1, 21.5), (9.8, 21.2), (9.8, 20.8));
        curve(&path, (9.8, 20.4), (10.1, 20.1), (10.5, 20.1));
        path.lineToPoint(point(11.3, 20.1));
        path.lineToPoint(point(11.3, 18.9));
        curve(&path, (9.4, 18.5), (8.1, 16.9), (8.1, 14.8));
        path.closePath();
        path.fill();
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(SIZE, SIZE), true, &drawing);
    image.setTemplate(true);
    image
}
