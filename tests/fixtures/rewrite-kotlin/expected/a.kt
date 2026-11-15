fun helper(x: Int): Int = x

fun main() {
    val a = renamed(1, 2)
    val b = renamed(x)
    val c = other(1)
    // helper(9) in comment
    println(a + b + c)
}
