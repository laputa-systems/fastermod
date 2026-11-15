class A {
	static int renamed(int x) { return x; }
	public static void main(String[] args) {
		int count = renamed(1);
	}
}
