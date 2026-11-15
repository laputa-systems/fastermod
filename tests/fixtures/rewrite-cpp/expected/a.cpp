int helper(int x) { return x; }
int main() {
  int a = renamed(1, 2);
  int b = renamed(x);
  int c = other(1);
  // helper(9) in comment
  return a + b + c;
}
