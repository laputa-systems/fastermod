class A {
  int field;
  int Boost(int step) {
    int total = field + step;
    for (int i = 0; i < step; i++) { total += i; }
    return total;
  }
  void Use() {
    int x = 1;
    x = Boost(x);
    this.field = x;
  }
}
