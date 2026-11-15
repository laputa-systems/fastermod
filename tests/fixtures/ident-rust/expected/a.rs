fn renamed(x: i32) -> i32 { x + 1 }
struct S { renamed: i32 }
fn main() {
    let renamed = 3;
    let s = S { renamed };
    println!("{} {} {}", renamed(renamed), s.renamed, myhelper);
    // helper in comment
    let t = "helper in string";
}
