int renamed(int x) { return x + 1; }
int main(void) {
  int count = renamed(1);
  // helper in comment
  char *s = "helper";
  return count + (int)(long)s[0];
}
