int helper(int x) { return x; }
int main(void) {
  renamed(1, 2);
  int a = helper(x);
  int c = other(1);
  // helper(9) in comment
  return a + c;
}
