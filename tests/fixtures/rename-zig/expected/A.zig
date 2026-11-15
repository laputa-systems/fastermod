fn renamed(x: i32) i32 {
    var y = x + 1;
    return y;
}
const count = renamed(1);
pub fn main() void {
    if (count > 0) {
        var helper = count;
        _ = helper;
    }
    _ = count;
}
