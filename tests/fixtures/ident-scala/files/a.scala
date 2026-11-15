object A {
  def helper(x: Int): Int = x + 1
  def main(args: Array[String]): Unit = {
    val count = helper(1)
    // helper in comment
    val s = "helper"
    println(count.toString + s)
  }
}
