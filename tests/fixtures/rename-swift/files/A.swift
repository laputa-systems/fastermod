class Foo {
  var field = 0
  func bump(_ step: Int) -> Int {
    let total = field + step
    return total
  }
  func use() {
    var bump = 1
    bump = bump + 1
    _ = bump
    var x = 1
    x = self.bump(x)
    self.field = x
  }
}
