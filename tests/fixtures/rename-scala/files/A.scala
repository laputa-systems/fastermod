object A {
  def helper(x: Int): Int = x + 1
  def main(args: Array[String]): Unit = {
    val count = helper(1)
    if (count > 0) {
      val helper = count
      println(helper)
    }
    println(count)
  }
}
