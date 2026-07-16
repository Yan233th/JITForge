import ast
import sys

import jitforge_http


FORBIDDEN_MODULES = {
    "ctypes", "http", "marshal", "multiprocessing", "os", "pathlib",
    "pickle", "shutil", "socket", "ssl", "subprocess", "tempfile", "urllib",
}
FORBIDDEN_CALLS = {"__import__", "compile", "eval", "exec", "open"}


class PolicyVisitor(ast.NodeVisitor):
    def visit_Import(self, node):
        for alias in node.names:
            if alias.name.split(".", 1)[0] in FORBIDDEN_MODULES:
                raise ValueError(f"forbidden import: {alias.name}")
        self.generic_visit(node)

    def visit_ImportFrom(self, node):
        if node.module and node.module.split(".", 1)[0] in FORBIDDEN_MODULES:
            raise ValueError(f"forbidden import: {node.module}")
        self.generic_visit(node)

    def visit_Call(self, node):
        if isinstance(node.func, ast.Name) and node.func.id in FORBIDDEN_CALLS:
            raise ValueError(f"forbidden call: {node.func.id}")
        self.generic_visit(node)


def main():
    if len(sys.argv) < 2:
        raise SystemExit("jitforge runtime: tool path is missing")
    tool_path = sys.argv[1]
    try:
        with open(tool_path, "r", encoding="utf-8") as source_file:
            source = source_file.read()
        tree = ast.parse(source, filename=tool_path, mode="exec")
        PolicyVisitor().visit(tree)
        code = compile(tree, tool_path, "exec")
    except (OSError, SyntaxError, ValueError) as error:
        print(f"jitforge runtime policy: {error}", file=sys.stderr)
        raise SystemExit(126)

    jitforge_http._install_audit_hook()
    sys.argv = [tool_path, *sys.argv[2:]]
    namespace = {
        "__builtins__": __builtins__,
        "__file__": tool_path,
        "__name__": "__main__",
        "__package__": None,
    }
    exec(code, namespace, namespace)


if __name__ == "__main__":
    main()
