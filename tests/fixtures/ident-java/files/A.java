class A {
  static int helper(int x) { return x + 1; }
  public static void main(String[] args) {
    int count = helper(1);
    // helper in comment
    String s = "helper";
    System.out.println(count + s);
  }
}
