class A {
  int field;
  int Bump(int step) {
    int total = field + step;
    for (int i = 0; i < step; i++) { total += i; }
    return total;
  }
  void Use() {
    int x = 1;
    x = Bump(x);
    this.field = x;
  }
}
