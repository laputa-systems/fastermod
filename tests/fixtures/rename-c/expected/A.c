int renamed(int x) {
  int y = x + 1;
  return y;
}
int count = renamed(1);
int main(void) {
  if (count > 0) {
    int helper = count;
    count = helper;
  }
  return count;
}
