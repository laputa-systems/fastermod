class A {
  static int renamed(int x) { return x + 1; }
  public static void main(String[] args) {
    int count = renamed(1);
    // helper in comment
    String s = "helper";
    System.out.println(count + s);
  }
}
