def helper(int x) {
    return x
}
def useIt() {
    def helper = 5
    return helper + outer(helper)
}
y = helper(1)
