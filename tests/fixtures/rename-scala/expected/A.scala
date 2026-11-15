object A {
  def renamed(x: Int): Int = x + 1
  def main(args: Array[String]): Unit = {
    val count = renamed(1)
    if (count > 0) {
      val helper = count
      println(helper)
    }
    println(count)
  }
}
