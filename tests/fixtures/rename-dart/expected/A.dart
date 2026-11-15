class Foo {
  var field = 0;
  int boost(int step) {
    var total = field + step;
    return total;
  }
  void use() {
    var bump = 1;
    bump = bump + 1;
    _ = bump;
    var x = this.boost(2);
    this.field = x;
  }
}
