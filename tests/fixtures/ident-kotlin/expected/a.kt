fun renamed(x: Int): Int = x + 1

fun main() {
    val count = renamed(1)
    // helper in comment
    val s = "helper"
    println(count.toString() + s)
}
