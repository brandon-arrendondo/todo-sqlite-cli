# Sphinx configuration for todo-sqlite-cli Guide

project = 'todo-sqlite-cli'
author = 'Brandon Arrendondo'
copyright = '2026, BISSELL Homecare, Inc.'

extensions = []

templates_path = []
exclude_patterns = []

# -- HTML output (sphinx-rtd-theme) --
html_theme = 'sphinx_rtd_theme'
html_static_path = []

# -- LaTeX / PDF output --
latex_elements = {
    'papersize': 'letterpaper',
    'pointsize': '10pt',
    'preamble': r'''
\usepackage{enumitem}
\setlistdepth{9}
''',
}

latex_documents = [
    ('index', 'todo-sqlite-cli-developer-guide.tex', 'todo-sqlite-cli Guide',
     'Brandon Arrendondo', 'manual'),
]
