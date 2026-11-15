class A {
  static int Helper(int x) { return x; }
  static void Main() {
    int a = Renamed(1, 2);
    int b = Renamed(x);
    int c = other(1);
    // Helper(9) in comment
    System.Console.WriteLine(a + b + c);
  }
}
