fn renamed(x: i32) i32 {
    return x + 1;
}

pub fn main() void {
    const count = renamed(1);
    // helper in comment
    const s = "helper";
    _ = count;
    _ = s;
}
