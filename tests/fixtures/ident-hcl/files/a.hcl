resource "aws_instance" "helper" {
  ami = var.bump
}
