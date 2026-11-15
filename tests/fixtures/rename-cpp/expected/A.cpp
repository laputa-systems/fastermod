class Foo {
public:
  int field = 0;
  int boost(int step) {
    int total = field + step;
    return total;
  }
  void use() {
    int bump = 1;
    bump = bump + 1;
    int x = this->boost(2);
    this->field = x;
  }
};
