fn helper(x: i32) -> i32 { x + 1 }
struct S { helper: i32 }
fn main() {
    let helper = 3;
    let s = S { helper };
    println!("{} {} {}", helper(helper), s.helper, myhelper);
    // helper in comment
    let t = "helper in string";
}
