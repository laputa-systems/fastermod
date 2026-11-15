int helper(int x) { return x + 1; }
int main() {
  int count = helper(1);
  // helper in comment
  char *s = "helper";
  return count + (int)(long)s[0];
}
