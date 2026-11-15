class A {
  static int helper(int x) { return x; }
  public static void main(String[] args) {
    int a = renamed(1, 2);
    int b = renamed(x);
    int c = other(1);
    // helper(9) in comment
    System.out.println(a + b + c);
  }
}
