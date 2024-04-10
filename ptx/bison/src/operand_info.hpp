#pragma once

#include <string>

#include "memory_space.hpp"
#include "operand_type.hpp"
#include "ptx_reg.hpp"

class gpgpu_context;
class symbol;

class operand_info {
public:
  operand_info(gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = false;
    m_immediate_address = false;
    m_addr_offset = 0;
    m_value.m_symbolic = NULL;
  }
  operand_info(const symbol *addr, gpgpu_context *ctx);

  operand_info(const symbol *addr1, const symbol *addr2, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_type = memory_t;
    m_value.m_vector_symbolic = new const symbol *[8];
    m_value.m_vector_symbolic[0] = addr1;
    m_value.m_vector_symbolic[1] = addr2;
    m_value.m_vector_symbolic[2] = NULL;
    m_value.m_vector_symbolic[3] = NULL;
    m_value.m_vector_symbolic[4] = NULL;
    m_value.m_vector_symbolic[5] = NULL;
    m_value.m_vector_symbolic[6] = NULL;
    m_value.m_vector_symbolic[7] = NULL;
    m_addr_offset = 0;
    m_vector = false;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }
  operand_info(int builtin_id, int dim_mod, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = false;
    m_type = builtin_t;
    m_value.m_int = builtin_id;
    m_addr_offset = dim_mod;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }
  operand_info(const symbol *addr, int offset, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = false;
    m_type = address_t;
    m_value.m_symbolic = addr;
    m_addr_offset = offset;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }
  operand_info(unsigned x, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = false;
    m_type = unsigned_t;
    m_value.m_unsigned = x;
    m_addr_offset = x;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = true;
  }
  operand_info(int x, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = false;
    m_type = int_t;
    m_value.m_int = x;
    m_addr_offset = 0;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }
  operand_info(float x, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = false;
    m_type = float_op_t;
    m_value.m_float = x;
    m_addr_offset = 0;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }
  operand_info(double x, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = false;
    m_type = double_op_t;
    m_value.m_double = x;
    m_addr_offset = 0;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }
  operand_info(const symbol *s1, const symbol *s2, const symbol *s3,
               const symbol *s4, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = true;
    m_type = vector_t;
    m_value.m_vector_symbolic = new const symbol *[8];
    m_value.m_vector_symbolic[0] = s1;
    m_value.m_vector_symbolic[1] = s2;
    m_value.m_vector_symbolic[2] = s3;
    m_value.m_vector_symbolic[3] = s4;
    m_value.m_vector_symbolic[4] = NULL;
    m_value.m_vector_symbolic[5] = NULL;
    m_value.m_vector_symbolic[6] = NULL;
    m_value.m_vector_symbolic[7] = NULL;
    m_addr_offset = 0;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }
  operand_info(const symbol *s1, const symbol *s2, const symbol *s3,
               const symbol *s4, const symbol *s5, const symbol *s6,
               const symbol *s7, const symbol *s8, gpgpu_context *ctx) {
    init(ctx);
    m_is_non_arch_reg = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = 0;
    m_uid = get_uid();
    m_valid = true;
    m_vector = true;
    m_type = vector_t;
    m_value.m_vector_symbolic = new const symbol *[8];
    m_value.m_vector_symbolic[0] = s1;
    m_value.m_vector_symbolic[1] = s2;
    m_value.m_vector_symbolic[2] = s3;
    m_value.m_vector_symbolic[3] = s4;
    m_value.m_vector_symbolic[4] = s5;
    m_value.m_vector_symbolic[5] = s6;
    m_value.m_vector_symbolic[6] = s7;
    m_value.m_vector_symbolic[7] = s8;
    m_addr_offset = 0;
    m_neg_pred = false;
    m_is_return_var = false;
    m_immediate_address = false;
  }

  void init(gpgpu_context *ctx) {
    gpgpu_ctx = ctx;
    m_uid = (unsigned)-1;
    m_valid = false;
    m_vector = false;
    m_type = undef_t;
    m_immediate_address = false;
    m_addr_space = undefined_space;
    m_operand_lohi = 0;
    m_double_operand_type = 0;
    m_operand_neg = false;
    m_const_mem_offset = (unsigned)-1;
    m_value.m_int = 0;
    m_value.m_unsigned = (unsigned)-1;
    m_value.m_float = 0;
    m_value.m_double = 0;
    for (unsigned i = 0; i < 4; i++) {
      m_value.m_vint[i] = 0;
      m_value.m_vunsigned[i] = 0;
      m_value.m_vfloat[i] = 0;
      m_value.m_vdouble[i] = 0;
    }
    m_value.m_symbolic = NULL;
    m_value.m_vector_symbolic = NULL;
    m_addr_offset = 0;
    m_neg_pred = 0;
    m_is_return_var = 0;
    m_is_non_arch_reg = 0;
  }
  void make_memory_operand() { m_type = memory_t; }
  void set_return() { m_is_return_var = true; }
  void set_immediate_addr() { m_immediate_address = true; }

  const std::string &name() const;

  unsigned get_vect_nelem() const {
    assert(is_vector());
    if (!m_value.m_vector_symbolic[0])
      return 0;
    if (!m_value.m_vector_symbolic[1])
      return 1;
    if (!m_value.m_vector_symbolic[2])
      return 2;
    if (!m_value.m_vector_symbolic[3])
      return 3;
    if (!m_value.m_vector_symbolic[4])
      return 4;
    if (!m_value.m_vector_symbolic[5])
      return 5;
    if (!m_value.m_vector_symbolic[6])
      return 6;
    if (!m_value.m_vector_symbolic[7])
      return 7;
    return 8;
  }

  const symbol *vec_symbol(int idx) const {
    assert(idx < 8);
    const symbol *result = m_value.m_vector_symbolic[idx];
    assert(result != NULL);
    return result;
  }

  const std::string &vec_name1() const;

  const std::string &vec_name2() const;

  const std::string &vec_name3() const;

  const std::string &vec_name4() const;

  bool is_reg() const;
  bool is_param_local() const;
  bool is_param_kernel() const;

  bool is_vector() const {
    if (m_vector)
      return true;
    return false;
  }
  int reg_num() const;
  int reg1_num() const;
  int reg2_num() const;
  int reg3_num() const;
  int reg4_num() const;
  int reg5_num() const;
  int reg6_num() const;
  int reg7_num() const;
  int reg8_num() const;
  int arch_reg_num() const;
  int arch_reg_num(unsigned n) const;

  bool is_label() const { return m_type == label_t; }
  bool is_builtin() const { return m_type == builtin_t; }

  // Memory operand used in ld / st instructions (ex. [__var1])
  bool is_memory_operand() const { return m_type == memory_t; }

  // Memory operand with immediate access (ex. s[0x0004] or g[$r1+=0x0004])
  // This is used by the PTXPlus extension. The operand is assigned an address
  // space during parsing.
  bool is_memory_operand2() const { return (m_addr_space != undefined_space); }

  bool is_immediate_address() const { return m_immediate_address; }

  bool is_literal() const {
    return m_type == int_t || m_type == float_op_t || m_type == double_op_t ||
           m_type == unsigned_t;
  }
  bool is_shared() const;
  bool is_sstarr() const;
  bool is_const() const;
  bool is_global() const;
  bool is_local() const;
  bool is_tex() const;
  bool is_return_var() const;

  bool is_function_address() const;

  ptx_reg_t get_literal_value() const {
    ptx_reg_t result;
    switch (m_type) {
    case int_t:
      result.s64 = m_value.m_int;
      break;
    case float_op_t:
      result.f32 = m_value.m_float;
      break;
    case double_op_t:
      result.f64 = m_value.m_double;
      break;
    case unsigned_t:
      result.u32 = m_value.m_unsigned;
      break;
    default:
      assert(0);
      break;
    }
    return result;
  }
  int get_int() const { return m_value.m_int; }
  int get_addr_offset() const { return m_addr_offset; }
  const symbol *get_symbol() const { return m_value.m_symbolic; }
  void set_type(enum operand_type type) { m_type = type; }
  enum operand_type get_type() const { return m_type; }
  void set_neg_pred() {
    assert(m_valid);
    m_neg_pred = true;
  }
  bool is_neg_pred() const { return m_neg_pred; }
  bool is_valid() const { return m_valid; }

  void set_addr_space(enum _memory_space_t set_value) {
    m_addr_space = set_value;
  }
  enum _memory_space_t get_addr_space() const { return m_addr_space; }
  void set_operand_lohi(int set_value) { m_operand_lohi = set_value; }
  int get_operand_lohi() const { return m_operand_lohi; }
  void set_double_operand_type(int set_value) {
    m_double_operand_type = set_value;
  }
  int get_double_operand_type() const { return m_double_operand_type; }
  void set_operand_neg() { m_operand_neg = true; }
  bool get_operand_neg() const { return m_operand_neg; }
  void set_const_mem_offset(addr_t set_value) {
    m_const_mem_offset = set_value;
  }
  addr_t get_const_mem_offset() const { return m_const_mem_offset; }
  bool is_non_arch_reg() const { return m_is_non_arch_reg; }

private:
  gpgpu_context *gpgpu_ctx;
  unsigned m_uid;
  bool m_valid;
  bool m_vector;
  enum operand_type m_type;
  bool m_immediate_address;
  enum _memory_space_t m_addr_space;
  int m_operand_lohi;
  int m_double_operand_type;
  bool m_operand_neg;
  addr_t m_const_mem_offset;
  union {
    int m_int;
    unsigned int m_unsigned;
    float m_float;
    double m_double;
    int m_vint[4];
    unsigned int m_vunsigned[4];
    float m_vfloat[4];
    double m_vdouble[4];
    const symbol *m_symbolic;
    const symbol **m_vector_symbolic;
  } m_value;

  int m_addr_offset;

  bool m_neg_pred;
  bool m_is_return_var;
  bool m_is_non_arch_reg;

  unsigned get_uid();
};
