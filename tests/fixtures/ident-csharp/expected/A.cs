class B {
  static int Renamed(int x) { return x + 1; }
  static void Main() {
    int count = Renamed(1);
    // Helper in comment
    string s = "Helper";
    System.Console.WriteLine(count + s);
  }
}
