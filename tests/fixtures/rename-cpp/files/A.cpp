class Foo {
public:
  int field = 0;
  int bump(int step) {
    int total = field + step;
    return total;
  }
  void use() {
    int bump = 1;
    bump = bump + 1;
    int x = this->bump(2);
    this->field = x;
  }
};
