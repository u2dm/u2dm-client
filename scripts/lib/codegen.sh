: "${U2DM_CODEGEN:=auto}"

cranelift_installed() {
  local sysroot
  sysroot=$(rustc +nightly --print sysroot 2>/dev/null) || return 1
  compgen -G "$sysroot/lib/rustlib/*/codegen-backends/librustc_codegen_cranelift*" >/dev/null
}

physical_cores() {
  local cores
  cores=$(lscpu -p=CORE,SOCKET 2>/dev/null | grep -v '^#' | sort -u | wc -l || true)
  if ((cores > 0)); then
    echo "$cores"
  else
    nproc
  fi
}

codegen_cargo() {
  local profile=$1 subcommand=$2 backend=$U2DM_CODEGEN package threads
  shift 2
  local -a toolchain=() codegen=()
  case $backend in
    auto)
      if cranelift_installed; then
        backend=cranelift
      else
        printf 'no nightly toolchain with rustc-codegen-cranelift: building with LLVM\n' >&2
        backend=llvm
      fi
      ;;
    cranelift)
      cranelift_installed || {
        printf 'Cranelift needs: rustup component add rustc-codegen-cranelift --toolchain nightly\n' >&2
        return 1
      }
      ;;
    llvm) ;;
    *)
      printf 'U2DM_CODEGEN must be auto, cranelift or llvm, not %s\n' "$backend" >&2
      return 1
      ;;
  esac
  if [[ $backend == cranelift ]]; then
    threads=$(physical_cores)
    toolchain=(+nightly)
    codegen=(-Zcodegen-backend -Zprofile-rustflags)
    for package in u2dm u2dm-ui; do
      codegen+=(--config "profile.$profile.package.$package.codegen-backend=\"cranelift\""
        --config "profile.$profile.package.$package.rustflags=[\"-Zthreads=$threads\"]")
    done
    codegen+=(--config "profile.$profile.package.aes.rustflags=[\"--cfg\", 'aes_backend=\"soft\"']")
  fi
  cargo "${toolchain[@]}" "$subcommand" --profile "$profile" "${codegen[@]}" "$@"
}
