mod m { pub fn target() {} }
use m::target;

fn main() {
    let value = 1;
    let doubled = value * 2;
    target();
    m::target();
}
