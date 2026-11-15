class Foo {
  var field = 0
  func boost(_ step: Int) -> Int {
    let total = field + step
    return total
  }
  func use() {
    var bump = 1
    bump = bump + 1
    _ = bump
    var x = 1
    x = self.boost(x)
    self.field = x
  }
}
