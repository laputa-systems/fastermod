class B {
  static int Helper(int x) { return x + 1; }
  static void Main() {
    int count = Helper(1);
    // Helper in comment
    string s = "Helper";
    System.Console.WriteLine(count + s);
  }
}
