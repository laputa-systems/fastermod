class Foo(val prop: Int) {
  var field = 0
  fun bump(step: Int): Int {
    val total = field + step
    for (i in 0..step) { println(total) }
    return total
  }
  fun use() {
    val x = 1
    x.let { bump(it) }
  }
}
