//! the headunit's sensor readings as text lines, in the format the README
//! documents. values with a known scale come out in plain units, the rest as
//! the headunit sent them. a value the headunit left out is `-`.

use aap::proto;

const DEGREES_E7: f64 = 1e7;
pub const DEGREES_E6: f64 = 1e6;
pub const E3: f64 = 1e3;
pub const METRES_E2: f64 = 1e2;
const ABSENT: &str = "-";

fn scaled(value: Option<impl Into<f64>>, scale: f64) -> String {
    value.map_or(ABSENT.into(), |value| (value.into() / scale).to_string())
}

fn raw(value: Option<i32>) -> String {
    value.map_or(ABSENT.into(), |value| value.to_string())
}

fn flag(value: Option<bool>) -> String {
    raw(value.map(i32::from))
}

fn location(fix: &proto::Location) -> String {
    let position = [scaled(fix.latitude, DEGREES_E7), scaled(fix.longitude, DEGREES_E7)];
    let rest = [
        scaled(fix.accuracy, E3),
        scaled(fix.altitude, METRES_E2),
        scaled(fix.speed, E3),
        scaled(fix.bearing, DEGREES_E6),
    ];
    format!("location {} {}", position.join(" "), rest.join(" "))
}

fn compass(reading: &proto::Compass) -> String {
    let angles = [reading.bearing, reading.pitch, reading.roll].map(|angle| scaled(angle, DEGREES_E6));
    format!("compass {}", angles.join(" "))
}

fn environment(reading: &proto::Environment) -> String {
    let values = [reading.temperature, reading.pressure, reading.rain].map(raw);
    format!("environment {}", values.join(" "))
}

/// one line per reading, in the order of the sensor types
pub fn lines(event: &proto::SensorEvent) -> Vec<String> {
    let mut lines: Vec<String> = event.location.iter().map(location).collect();
    lines.extend(event.compass.iter().map(compass));
    lines.extend(event.speed.iter().map(|s| format!("speed {}", scaled(s.speed, E3))));
    lines.extend(event.rpm.iter().map(|r| format!("rpm {}", raw(r.rpm))));
    lines.extend(event.odometer.iter().map(|o| format!("odometer {} {}", raw(o.total), raw(o.trip))));
    lines.extend(event.fuel_level.iter().map(|f| format!("fuel {} {} {}", raw(f.level), raw(f.range), flag(f.low))));
    lines.extend(event.parking_brake.iter().map(|p| format!("parking-brake {}", flag(p.engaged))));
    lines.extend(event.gear.iter().map(|g| format!("gear {}", raw(g.gear))));
    lines.extend(event.night_mode.iter().map(|n| format!("night {}", flag(n.is_night))));
    lines.extend(event.environment.iter().map(environment));
    lines.extend(event.driving_status.iter().map(|d| format!("driving {}", raw(d.status))));
    lines
}
