;;; vfl.el --- vfl language server for VHDL, Verilog and SystemVerilog -*- lexical-binding: t; -*-

;; URL: https://github.com/thewenneur/vhdl-fmt-lint
;; Package-Requires: ((emacs "29.1"))
;; SPDX-License-Identifier: MIT OR Apache-2.0

;;; Commentary:

;; Registers `vfl lsp' with Eglot and with lsp-mode, for whichever of them
;; is loaded, in the VHDL and Verilog major modes (built in and tree-sitter
;; ones; Emacs opens .sv files in `verilog-mode').
;;
;;   (add-to-list 'load-path "/path/to/vhdl-fmt-lint/editors/emacs")
;;   (require 'vfl)
;;   (add-hook 'vhdl-mode-hook #'eglot-ensure)     ; or #'lsp-deferred
;;   (add-hook 'verilog-mode-hook #'eglot-ensure)
;;
;; See docs/editors.md.

;;; Code:

(defgroup vfl nil
  "The vfl language server."
  :group 'tools
  :prefix "vfl-")

(defcustom vfl-server-command '("vfl" "lsp")
  "Command that starts the vfl language server, with its arguments."
  :type '(repeat string))

(defconst vfl-major-modes
  '(vhdl-mode vhdl-ts-mode verilog-mode verilog-ts-mode)
  "Major modes the vfl language server is used in.")

(with-eval-after-load 'eglot
  (defvar eglot-server-programs)
  (add-to-list 'eglot-server-programs
               `(,vfl-major-modes . ,(lambda (&rest _) vfl-server-command))))

(declare-function lsp-register-client "lsp-mode")
(declare-function make-lsp-client "lsp-mode")
(declare-function lsp-stdio-connection "lsp-mode")
(declare-function lsp-activate-on "lsp-mode")
(defvar lsp-language-id-configuration)

(with-eval-after-load 'lsp-mode
  (dolist (mode '((vhdl-ts-mode . "vhdl") (verilog-ts-mode . "verilog")))
    (add-to-list 'lsp-language-id-configuration mode))
  (lsp-register-client
   (make-lsp-client
    :new-connection (lsp-stdio-connection (lambda () vfl-server-command))
    :activation-fn (lsp-activate-on "vhdl" "verilog" "systemverilog")
    ;; Above lsp-mode's own VHDL and Verilog clients.
    :priority 1
    :server-id 'vfl)))

(provide 'vfl)
;;; vfl.el ends here
