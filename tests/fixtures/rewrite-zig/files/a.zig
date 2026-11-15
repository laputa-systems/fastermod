fn helper(x: i32, y: i32) i32 {
    return x + y;
}

pub fn main() void {
    const a = helper(1, 2);
    const b = helper(x, y);
    const c = other(1);
    // helper(9) in comment
    _ = a + b + c;
}
