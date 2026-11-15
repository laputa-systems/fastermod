package main

func renamed(x int) int { return x }

func main() {
	count := renamed(1)
	_ = count
	// helper in comment
	s := "helper in string"
	_ = s
}
