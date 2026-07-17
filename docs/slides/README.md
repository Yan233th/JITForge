# JITForge Slides

`jitforge.md` 是 Slides 源文件。`index.html` 由 Marp 生成，供 Nginx 在 `/slides/` 下直接返回，文件已加入 `.gitignore`。

生成网页：

```bash
cd docs/slides
bunx @marp-team/marp-cli jitforge.md --html -o index.html
```

本机预览：

```bash
python3 -m http.server 39080 --bind ::
```

Nginx 只读挂载本目录；重新生成 `index.html` 后，线上内容会立即更新，不需要重建镜像。
