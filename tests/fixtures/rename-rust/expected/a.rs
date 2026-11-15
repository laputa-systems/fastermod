mod m { pub fn renamed() {} }
use m::renamed as target;

fn main() {
    let value = 1;
    let doubled = value * 2;
    target();
    m::renamed();
}
