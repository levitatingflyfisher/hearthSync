// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'kernel.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$ApiError {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError()';
}


}

/// @nodoc
class $ApiErrorCopyWith<$Res>  {
$ApiErrorCopyWith(ApiError _, $Res Function(ApiError) __);
}


/// Adds pattern-matching-related methods to [ApiError].
extension ApiErrorPatterns on ApiError {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ApiError_BadArgument value)?  badArgument,TResult Function( ApiError_Schema value)?  schema,TResult Function( ApiError_Undeclared value)?  undeclared,TResult Function( ApiError_Persist value)?  persist,TResult Function( ApiError_BadMessage value)?  badMessage,TResult Function( ApiError_NoKeys value)?  noKeys,TResult Function( ApiError_ClockBehind value)?  clockBehind,TResult Function( ApiError_Rejected value)?  rejected,TResult Function( ApiError_BadSnapshot value)?  badSnapshot,TResult Function( ApiError_SnapshotUnavailable value)?  snapshotUnavailable,TResult Function( ApiError_BadSignature value)?  badSignature,TResult Function( ApiError_NothingToFinish value)?  nothingToFinish,TResult Function( ApiError_AwaitingSignature value)?  awaitingSignature,TResult Function( ApiError_StaleGeneration value)?  staleGeneration,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ApiError_BadArgument() when badArgument != null:
return badArgument(_that);case ApiError_Schema() when schema != null:
return schema(_that);case ApiError_Undeclared() when undeclared != null:
return undeclared(_that);case ApiError_Persist() when persist != null:
return persist(_that);case ApiError_BadMessage() when badMessage != null:
return badMessage(_that);case ApiError_NoKeys() when noKeys != null:
return noKeys(_that);case ApiError_ClockBehind() when clockBehind != null:
return clockBehind(_that);case ApiError_Rejected() when rejected != null:
return rejected(_that);case ApiError_BadSnapshot() when badSnapshot != null:
return badSnapshot(_that);case ApiError_SnapshotUnavailable() when snapshotUnavailable != null:
return snapshotUnavailable(_that);case ApiError_BadSignature() when badSignature != null:
return badSignature(_that);case ApiError_NothingToFinish() when nothingToFinish != null:
return nothingToFinish(_that);case ApiError_AwaitingSignature() when awaitingSignature != null:
return awaitingSignature(_that);case ApiError_StaleGeneration() when staleGeneration != null:
return staleGeneration(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ApiError_BadArgument value)  badArgument,required TResult Function( ApiError_Schema value)  schema,required TResult Function( ApiError_Undeclared value)  undeclared,required TResult Function( ApiError_Persist value)  persist,required TResult Function( ApiError_BadMessage value)  badMessage,required TResult Function( ApiError_NoKeys value)  noKeys,required TResult Function( ApiError_ClockBehind value)  clockBehind,required TResult Function( ApiError_Rejected value)  rejected,required TResult Function( ApiError_BadSnapshot value)  badSnapshot,required TResult Function( ApiError_SnapshotUnavailable value)  snapshotUnavailable,required TResult Function( ApiError_BadSignature value)  badSignature,required TResult Function( ApiError_NothingToFinish value)  nothingToFinish,required TResult Function( ApiError_AwaitingSignature value)  awaitingSignature,required TResult Function( ApiError_StaleGeneration value)  staleGeneration,}){
final _that = this;
switch (_that) {
case ApiError_BadArgument():
return badArgument(_that);case ApiError_Schema():
return schema(_that);case ApiError_Undeclared():
return undeclared(_that);case ApiError_Persist():
return persist(_that);case ApiError_BadMessage():
return badMessage(_that);case ApiError_NoKeys():
return noKeys(_that);case ApiError_ClockBehind():
return clockBehind(_that);case ApiError_Rejected():
return rejected(_that);case ApiError_BadSnapshot():
return badSnapshot(_that);case ApiError_SnapshotUnavailable():
return snapshotUnavailable(_that);case ApiError_BadSignature():
return badSignature(_that);case ApiError_NothingToFinish():
return nothingToFinish(_that);case ApiError_AwaitingSignature():
return awaitingSignature(_that);case ApiError_StaleGeneration():
return staleGeneration(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ApiError_BadArgument value)?  badArgument,TResult? Function( ApiError_Schema value)?  schema,TResult? Function( ApiError_Undeclared value)?  undeclared,TResult? Function( ApiError_Persist value)?  persist,TResult? Function( ApiError_BadMessage value)?  badMessage,TResult? Function( ApiError_NoKeys value)?  noKeys,TResult? Function( ApiError_ClockBehind value)?  clockBehind,TResult? Function( ApiError_Rejected value)?  rejected,TResult? Function( ApiError_BadSnapshot value)?  badSnapshot,TResult? Function( ApiError_SnapshotUnavailable value)?  snapshotUnavailable,TResult? Function( ApiError_BadSignature value)?  badSignature,TResult? Function( ApiError_NothingToFinish value)?  nothingToFinish,TResult? Function( ApiError_AwaitingSignature value)?  awaitingSignature,TResult? Function( ApiError_StaleGeneration value)?  staleGeneration,}){
final _that = this;
switch (_that) {
case ApiError_BadArgument() when badArgument != null:
return badArgument(_that);case ApiError_Schema() when schema != null:
return schema(_that);case ApiError_Undeclared() when undeclared != null:
return undeclared(_that);case ApiError_Persist() when persist != null:
return persist(_that);case ApiError_BadMessage() when badMessage != null:
return badMessage(_that);case ApiError_NoKeys() when noKeys != null:
return noKeys(_that);case ApiError_ClockBehind() when clockBehind != null:
return clockBehind(_that);case ApiError_Rejected() when rejected != null:
return rejected(_that);case ApiError_BadSnapshot() when badSnapshot != null:
return badSnapshot(_that);case ApiError_SnapshotUnavailable() when snapshotUnavailable != null:
return snapshotUnavailable(_that);case ApiError_BadSignature() when badSignature != null:
return badSignature(_that);case ApiError_NothingToFinish() when nothingToFinish != null:
return nothingToFinish(_that);case ApiError_AwaitingSignature() when awaitingSignature != null:
return awaitingSignature(_that);case ApiError_StaleGeneration() when staleGeneration != null:
return staleGeneration(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String field0)?  badArgument,TResult Function( String field0)?  schema,TResult Function()?  undeclared,TResult Function( String field0)?  persist,TResult Function()?  badMessage,TResult Function()?  noKeys,TResult Function( BigInt now,  BigInt latest)?  clockBehind,TResult Function( String field0)?  rejected,TResult Function( String field0)?  badSnapshot,TResult Function()?  snapshotUnavailable,TResult Function()?  badSignature,TResult Function()?  nothingToFinish,TResult Function()?  awaitingSignature,TResult Function()?  staleGeneration,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ApiError_BadArgument() when badArgument != null:
return badArgument(_that.field0);case ApiError_Schema() when schema != null:
return schema(_that.field0);case ApiError_Undeclared() when undeclared != null:
return undeclared();case ApiError_Persist() when persist != null:
return persist(_that.field0);case ApiError_BadMessage() when badMessage != null:
return badMessage();case ApiError_NoKeys() when noKeys != null:
return noKeys();case ApiError_ClockBehind() when clockBehind != null:
return clockBehind(_that.now,_that.latest);case ApiError_Rejected() when rejected != null:
return rejected(_that.field0);case ApiError_BadSnapshot() when badSnapshot != null:
return badSnapshot(_that.field0);case ApiError_SnapshotUnavailable() when snapshotUnavailable != null:
return snapshotUnavailable();case ApiError_BadSignature() when badSignature != null:
return badSignature();case ApiError_NothingToFinish() when nothingToFinish != null:
return nothingToFinish();case ApiError_AwaitingSignature() when awaitingSignature != null:
return awaitingSignature();case ApiError_StaleGeneration() when staleGeneration != null:
return staleGeneration();case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String field0)  badArgument,required TResult Function( String field0)  schema,required TResult Function()  undeclared,required TResult Function( String field0)  persist,required TResult Function()  badMessage,required TResult Function()  noKeys,required TResult Function( BigInt now,  BigInt latest)  clockBehind,required TResult Function( String field0)  rejected,required TResult Function( String field0)  badSnapshot,required TResult Function()  snapshotUnavailable,required TResult Function()  badSignature,required TResult Function()  nothingToFinish,required TResult Function()  awaitingSignature,required TResult Function()  staleGeneration,}) {final _that = this;
switch (_that) {
case ApiError_BadArgument():
return badArgument(_that.field0);case ApiError_Schema():
return schema(_that.field0);case ApiError_Undeclared():
return undeclared();case ApiError_Persist():
return persist(_that.field0);case ApiError_BadMessage():
return badMessage();case ApiError_NoKeys():
return noKeys();case ApiError_ClockBehind():
return clockBehind(_that.now,_that.latest);case ApiError_Rejected():
return rejected(_that.field0);case ApiError_BadSnapshot():
return badSnapshot(_that.field0);case ApiError_SnapshotUnavailable():
return snapshotUnavailable();case ApiError_BadSignature():
return badSignature();case ApiError_NothingToFinish():
return nothingToFinish();case ApiError_AwaitingSignature():
return awaitingSignature();case ApiError_StaleGeneration():
return staleGeneration();}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String field0)?  badArgument,TResult? Function( String field0)?  schema,TResult? Function()?  undeclared,TResult? Function( String field0)?  persist,TResult? Function()?  badMessage,TResult? Function()?  noKeys,TResult? Function( BigInt now,  BigInt latest)?  clockBehind,TResult? Function( String field0)?  rejected,TResult? Function( String field0)?  badSnapshot,TResult? Function()?  snapshotUnavailable,TResult? Function()?  badSignature,TResult? Function()?  nothingToFinish,TResult? Function()?  awaitingSignature,TResult? Function()?  staleGeneration,}) {final _that = this;
switch (_that) {
case ApiError_BadArgument() when badArgument != null:
return badArgument(_that.field0);case ApiError_Schema() when schema != null:
return schema(_that.field0);case ApiError_Undeclared() when undeclared != null:
return undeclared();case ApiError_Persist() when persist != null:
return persist(_that.field0);case ApiError_BadMessage() when badMessage != null:
return badMessage();case ApiError_NoKeys() when noKeys != null:
return noKeys();case ApiError_ClockBehind() when clockBehind != null:
return clockBehind(_that.now,_that.latest);case ApiError_Rejected() when rejected != null:
return rejected(_that.field0);case ApiError_BadSnapshot() when badSnapshot != null:
return badSnapshot(_that.field0);case ApiError_SnapshotUnavailable() when snapshotUnavailable != null:
return snapshotUnavailable();case ApiError_BadSignature() when badSignature != null:
return badSignature();case ApiError_NothingToFinish() when nothingToFinish != null:
return nothingToFinish();case ApiError_AwaitingSignature() when awaitingSignature != null:
return awaitingSignature();case ApiError_StaleGeneration() when staleGeneration != null:
return staleGeneration();case _:
  return null;

}
}

}

/// @nodoc


class ApiError_BadArgument extends ApiError {
  const ApiError_BadArgument(this.field0): super._();
  

 final  String field0;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ApiError_BadArgumentCopyWith<ApiError_BadArgument> get copyWith => _$ApiError_BadArgumentCopyWithImpl<ApiError_BadArgument>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_BadArgument&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'ApiError.badArgument(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $ApiError_BadArgumentCopyWith<$Res> implements $ApiErrorCopyWith<$Res> {
  factory $ApiError_BadArgumentCopyWith(ApiError_BadArgument value, $Res Function(ApiError_BadArgument) _then) = _$ApiError_BadArgumentCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$ApiError_BadArgumentCopyWithImpl<$Res>
    implements $ApiError_BadArgumentCopyWith<$Res> {
  _$ApiError_BadArgumentCopyWithImpl(this._self, this._then);

  final ApiError_BadArgument _self;
  final $Res Function(ApiError_BadArgument) _then;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(ApiError_BadArgument(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ApiError_Schema extends ApiError {
  const ApiError_Schema(this.field0): super._();
  

 final  String field0;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ApiError_SchemaCopyWith<ApiError_Schema> get copyWith => _$ApiError_SchemaCopyWithImpl<ApiError_Schema>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_Schema&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'ApiError.schema(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $ApiError_SchemaCopyWith<$Res> implements $ApiErrorCopyWith<$Res> {
  factory $ApiError_SchemaCopyWith(ApiError_Schema value, $Res Function(ApiError_Schema) _then) = _$ApiError_SchemaCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$ApiError_SchemaCopyWithImpl<$Res>
    implements $ApiError_SchemaCopyWith<$Res> {
  _$ApiError_SchemaCopyWithImpl(this._self, this._then);

  final ApiError_Schema _self;
  final $Res Function(ApiError_Schema) _then;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(ApiError_Schema(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ApiError_Undeclared extends ApiError {
  const ApiError_Undeclared(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_Undeclared);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.undeclared()';
}


}




/// @nodoc


class ApiError_Persist extends ApiError {
  const ApiError_Persist(this.field0): super._();
  

 final  String field0;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ApiError_PersistCopyWith<ApiError_Persist> get copyWith => _$ApiError_PersistCopyWithImpl<ApiError_Persist>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_Persist&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'ApiError.persist(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $ApiError_PersistCopyWith<$Res> implements $ApiErrorCopyWith<$Res> {
  factory $ApiError_PersistCopyWith(ApiError_Persist value, $Res Function(ApiError_Persist) _then) = _$ApiError_PersistCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$ApiError_PersistCopyWithImpl<$Res>
    implements $ApiError_PersistCopyWith<$Res> {
  _$ApiError_PersistCopyWithImpl(this._self, this._then);

  final ApiError_Persist _self;
  final $Res Function(ApiError_Persist) _then;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(ApiError_Persist(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ApiError_BadMessage extends ApiError {
  const ApiError_BadMessage(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_BadMessage);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.badMessage()';
}


}




/// @nodoc


class ApiError_NoKeys extends ApiError {
  const ApiError_NoKeys(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_NoKeys);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.noKeys()';
}


}




/// @nodoc


class ApiError_ClockBehind extends ApiError {
  const ApiError_ClockBehind({required this.now, required this.latest}): super._();
  

 final  BigInt now;
 final  BigInt latest;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ApiError_ClockBehindCopyWith<ApiError_ClockBehind> get copyWith => _$ApiError_ClockBehindCopyWithImpl<ApiError_ClockBehind>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_ClockBehind&&(identical(other.now, now) || other.now == now)&&(identical(other.latest, latest) || other.latest == latest));
}


@override
int get hashCode => Object.hash(runtimeType,now,latest);

@override
String toString() {
  return 'ApiError.clockBehind(now: $now, latest: $latest)';
}


}

/// @nodoc
abstract mixin class $ApiError_ClockBehindCopyWith<$Res> implements $ApiErrorCopyWith<$Res> {
  factory $ApiError_ClockBehindCopyWith(ApiError_ClockBehind value, $Res Function(ApiError_ClockBehind) _then) = _$ApiError_ClockBehindCopyWithImpl;
@useResult
$Res call({
 BigInt now, BigInt latest
});




}
/// @nodoc
class _$ApiError_ClockBehindCopyWithImpl<$Res>
    implements $ApiError_ClockBehindCopyWith<$Res> {
  _$ApiError_ClockBehindCopyWithImpl(this._self, this._then);

  final ApiError_ClockBehind _self;
  final $Res Function(ApiError_ClockBehind) _then;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? now = null,Object? latest = null,}) {
  return _then(ApiError_ClockBehind(
now: null == now ? _self.now : now // ignore: cast_nullable_to_non_nullable
as BigInt,latest: null == latest ? _self.latest : latest // ignore: cast_nullable_to_non_nullable
as BigInt,
  ));
}


}

/// @nodoc


class ApiError_Rejected extends ApiError {
  const ApiError_Rejected(this.field0): super._();
  

 final  String field0;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ApiError_RejectedCopyWith<ApiError_Rejected> get copyWith => _$ApiError_RejectedCopyWithImpl<ApiError_Rejected>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_Rejected&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'ApiError.rejected(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $ApiError_RejectedCopyWith<$Res> implements $ApiErrorCopyWith<$Res> {
  factory $ApiError_RejectedCopyWith(ApiError_Rejected value, $Res Function(ApiError_Rejected) _then) = _$ApiError_RejectedCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$ApiError_RejectedCopyWithImpl<$Res>
    implements $ApiError_RejectedCopyWith<$Res> {
  _$ApiError_RejectedCopyWithImpl(this._self, this._then);

  final ApiError_Rejected _self;
  final $Res Function(ApiError_Rejected) _then;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(ApiError_Rejected(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ApiError_BadSnapshot extends ApiError {
  const ApiError_BadSnapshot(this.field0): super._();
  

 final  String field0;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ApiError_BadSnapshotCopyWith<ApiError_BadSnapshot> get copyWith => _$ApiError_BadSnapshotCopyWithImpl<ApiError_BadSnapshot>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_BadSnapshot&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'ApiError.badSnapshot(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $ApiError_BadSnapshotCopyWith<$Res> implements $ApiErrorCopyWith<$Res> {
  factory $ApiError_BadSnapshotCopyWith(ApiError_BadSnapshot value, $Res Function(ApiError_BadSnapshot) _then) = _$ApiError_BadSnapshotCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$ApiError_BadSnapshotCopyWithImpl<$Res>
    implements $ApiError_BadSnapshotCopyWith<$Res> {
  _$ApiError_BadSnapshotCopyWithImpl(this._self, this._then);

  final ApiError_BadSnapshot _self;
  final $Res Function(ApiError_BadSnapshot) _then;

/// Create a copy of ApiError
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(ApiError_BadSnapshot(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ApiError_SnapshotUnavailable extends ApiError {
  const ApiError_SnapshotUnavailable(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_SnapshotUnavailable);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.snapshotUnavailable()';
}


}




/// @nodoc


class ApiError_BadSignature extends ApiError {
  const ApiError_BadSignature(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_BadSignature);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.badSignature()';
}


}




/// @nodoc


class ApiError_NothingToFinish extends ApiError {
  const ApiError_NothingToFinish(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_NothingToFinish);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.nothingToFinish()';
}


}




/// @nodoc


class ApiError_AwaitingSignature extends ApiError {
  const ApiError_AwaitingSignature(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_AwaitingSignature);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.awaitingSignature()';
}


}




/// @nodoc


class ApiError_StaleGeneration extends ApiError {
  const ApiError_StaleGeneration(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is ApiError_StaleGeneration);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'ApiError.staleGeneration()';
}


}




/// @nodoc
mixin _$KernelValue {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is KernelValue);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'KernelValue()';
}


}

/// @nodoc
class $KernelValueCopyWith<$Res>  {
$KernelValueCopyWith(KernelValue _, $Res Function(KernelValue) __);
}


/// Adds pattern-matching-related methods to [KernelValue].
extension KernelValuePatterns on KernelValue {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( KernelValue_Null value)?  null_,TResult Function( KernelValue_Bool value)?  bool,TResult Function( KernelValue_Int value)?  int,TResult Function( KernelValue_Text value)?  text,TResult Function( KernelValue_Bytes value)?  bytes,required TResult orElse(),}){
final _that = this;
switch (_that) {
case KernelValue_Null() when null_ != null:
return null_(_that);case KernelValue_Bool() when bool != null:
return bool(_that);case KernelValue_Int() when int != null:
return int(_that);case KernelValue_Text() when text != null:
return text(_that);case KernelValue_Bytes() when bytes != null:
return bytes(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( KernelValue_Null value)  null_,required TResult Function( KernelValue_Bool value)  bool,required TResult Function( KernelValue_Int value)  int,required TResult Function( KernelValue_Text value)  text,required TResult Function( KernelValue_Bytes value)  bytes,}){
final _that = this;
switch (_that) {
case KernelValue_Null():
return null_(_that);case KernelValue_Bool():
return bool(_that);case KernelValue_Int():
return int(_that);case KernelValue_Text():
return text(_that);case KernelValue_Bytes():
return bytes(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( KernelValue_Null value)?  null_,TResult? Function( KernelValue_Bool value)?  bool,TResult? Function( KernelValue_Int value)?  int,TResult? Function( KernelValue_Text value)?  text,TResult? Function( KernelValue_Bytes value)?  bytes,}){
final _that = this;
switch (_that) {
case KernelValue_Null() when null_ != null:
return null_(_that);case KernelValue_Bool() when bool != null:
return bool(_that);case KernelValue_Int() when int != null:
return int(_that);case KernelValue_Text() when text != null:
return text(_that);case KernelValue_Bytes() when bytes != null:
return bytes(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  null_,TResult Function( bool field0)?  bool,TResult Function( PlatformInt64 field0)?  int,TResult Function( String field0)?  text,TResult Function( Uint8List field0)?  bytes,required TResult orElse(),}) {final _that = this;
switch (_that) {
case KernelValue_Null() when null_ != null:
return null_();case KernelValue_Bool() when bool != null:
return bool(_that.field0);case KernelValue_Int() when int != null:
return int(_that.field0);case KernelValue_Text() when text != null:
return text(_that.field0);case KernelValue_Bytes() when bytes != null:
return bytes(_that.field0);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  null_,required TResult Function( bool field0)  bool,required TResult Function( PlatformInt64 field0)  int,required TResult Function( String field0)  text,required TResult Function( Uint8List field0)  bytes,}) {final _that = this;
switch (_that) {
case KernelValue_Null():
return null_();case KernelValue_Bool():
return bool(_that.field0);case KernelValue_Int():
return int(_that.field0);case KernelValue_Text():
return text(_that.field0);case KernelValue_Bytes():
return bytes(_that.field0);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  null_,TResult? Function( bool field0)?  bool,TResult? Function( PlatformInt64 field0)?  int,TResult? Function( String field0)?  text,TResult? Function( Uint8List field0)?  bytes,}) {final _that = this;
switch (_that) {
case KernelValue_Null() when null_ != null:
return null_();case KernelValue_Bool() when bool != null:
return bool(_that.field0);case KernelValue_Int() when int != null:
return int(_that.field0);case KernelValue_Text() when text != null:
return text(_that.field0);case KernelValue_Bytes() when bytes != null:
return bytes(_that.field0);case _:
  return null;

}
}

}

/// @nodoc


class KernelValue_Null extends KernelValue {
  const KernelValue_Null(): super._();
  






@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is KernelValue_Null);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'KernelValue.null_()';
}


}




/// @nodoc


class KernelValue_Bool extends KernelValue {
  const KernelValue_Bool(this.field0): super._();
  

 final  bool field0;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$KernelValue_BoolCopyWith<KernelValue_Bool> get copyWith => _$KernelValue_BoolCopyWithImpl<KernelValue_Bool>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is KernelValue_Bool&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'KernelValue.bool(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $KernelValue_BoolCopyWith<$Res> implements $KernelValueCopyWith<$Res> {
  factory $KernelValue_BoolCopyWith(KernelValue_Bool value, $Res Function(KernelValue_Bool) _then) = _$KernelValue_BoolCopyWithImpl;
@useResult
$Res call({
 bool field0
});




}
/// @nodoc
class _$KernelValue_BoolCopyWithImpl<$Res>
    implements $KernelValue_BoolCopyWith<$Res> {
  _$KernelValue_BoolCopyWithImpl(this._self, this._then);

  final KernelValue_Bool _self;
  final $Res Function(KernelValue_Bool) _then;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(KernelValue_Bool(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class KernelValue_Int extends KernelValue {
  const KernelValue_Int(this.field0): super._();
  

 final  PlatformInt64 field0;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$KernelValue_IntCopyWith<KernelValue_Int> get copyWith => _$KernelValue_IntCopyWithImpl<KernelValue_Int>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is KernelValue_Int&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'KernelValue.int(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $KernelValue_IntCopyWith<$Res> implements $KernelValueCopyWith<$Res> {
  factory $KernelValue_IntCopyWith(KernelValue_Int value, $Res Function(KernelValue_Int) _then) = _$KernelValue_IntCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 field0
});




}
/// @nodoc
class _$KernelValue_IntCopyWithImpl<$Res>
    implements $KernelValue_IntCopyWith<$Res> {
  _$KernelValue_IntCopyWithImpl(this._self, this._then);

  final KernelValue_Int _self;
  final $Res Function(KernelValue_Int) _then;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(KernelValue_Int(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class KernelValue_Text extends KernelValue {
  const KernelValue_Text(this.field0): super._();
  

 final  String field0;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$KernelValue_TextCopyWith<KernelValue_Text> get copyWith => _$KernelValue_TextCopyWithImpl<KernelValue_Text>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is KernelValue_Text&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'KernelValue.text(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $KernelValue_TextCopyWith<$Res> implements $KernelValueCopyWith<$Res> {
  factory $KernelValue_TextCopyWith(KernelValue_Text value, $Res Function(KernelValue_Text) _then) = _$KernelValue_TextCopyWithImpl;
@useResult
$Res call({
 String field0
});




}
/// @nodoc
class _$KernelValue_TextCopyWithImpl<$Res>
    implements $KernelValue_TextCopyWith<$Res> {
  _$KernelValue_TextCopyWithImpl(this._self, this._then);

  final KernelValue_Text _self;
  final $Res Function(KernelValue_Text) _then;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(KernelValue_Text(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class KernelValue_Bytes extends KernelValue {
  const KernelValue_Bytes(this.field0): super._();
  

 final  Uint8List field0;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$KernelValue_BytesCopyWith<KernelValue_Bytes> get copyWith => _$KernelValue_BytesCopyWithImpl<KernelValue_Bytes>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is KernelValue_Bytes&&const DeepCollectionEquality().equals(other.field0, field0));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(field0));

@override
String toString() {
  return 'KernelValue.bytes(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $KernelValue_BytesCopyWith<$Res> implements $KernelValueCopyWith<$Res> {
  factory $KernelValue_BytesCopyWith(KernelValue_Bytes value, $Res Function(KernelValue_Bytes) _then) = _$KernelValue_BytesCopyWithImpl;
@useResult
$Res call({
 Uint8List field0
});




}
/// @nodoc
class _$KernelValue_BytesCopyWithImpl<$Res>
    implements $KernelValue_BytesCopyWith<$Res> {
  _$KernelValue_BytesCopyWithImpl(this._self, this._then);

  final KernelValue_Bytes _self;
  final $Res Function(KernelValue_Bytes) _then;

/// Create a copy of KernelValue
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(KernelValue_Bytes(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc
mixin _$Merge {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Merge);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'Merge()';
}


}

/// @nodoc
class $MergeCopyWith<$Res>  {
$MergeCopyWith(Merge _, $Res Function(Merge) __);
}


/// Adds pattern-matching-related methods to [Merge].
extension MergePatterns on Merge {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Merge_Lww value)?  lww,TResult Function( Merge_AddWinsSet value)?  addWinsSet,TResult Function( Merge_AppendOnly value)?  appendOnly,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Merge_Lww() when lww != null:
return lww(_that);case Merge_AddWinsSet() when addWinsSet != null:
return addWinsSet(_that);case Merge_AppendOnly() when appendOnly != null:
return appendOnly(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Merge_Lww value)  lww,required TResult Function( Merge_AddWinsSet value)  addWinsSet,required TResult Function( Merge_AppendOnly value)  appendOnly,}){
final _that = this;
switch (_that) {
case Merge_Lww():
return lww(_that);case Merge_AddWinsSet():
return addWinsSet(_that);case Merge_AppendOnly():
return appendOnly(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Merge_Lww value)?  lww,TResult? Function( Merge_AddWinsSet value)?  addWinsSet,TResult? Function( Merge_AppendOnly value)?  appendOnly,}){
final _that = this;
switch (_that) {
case Merge_Lww() when lww != null:
return lww(_that);case Merge_AddWinsSet() when addWinsSet != null:
return addWinsSet(_that);case Merge_AppendOnly() when appendOnly != null:
return appendOnly(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( List<FieldDef> fields,  ContainerDef? container)?  lww,TResult Function( ValueType element)?  addWinsSet,TResult Function( ValueType record)?  appendOnly,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Merge_Lww() when lww != null:
return lww(_that.fields,_that.container);case Merge_AddWinsSet() when addWinsSet != null:
return addWinsSet(_that.element);case Merge_AppendOnly() when appendOnly != null:
return appendOnly(_that.record);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( List<FieldDef> fields,  ContainerDef? container)  lww,required TResult Function( ValueType element)  addWinsSet,required TResult Function( ValueType record)  appendOnly,}) {final _that = this;
switch (_that) {
case Merge_Lww():
return lww(_that.fields,_that.container);case Merge_AddWinsSet():
return addWinsSet(_that.element);case Merge_AppendOnly():
return appendOnly(_that.record);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( List<FieldDef> fields,  ContainerDef? container)?  lww,TResult? Function( ValueType element)?  addWinsSet,TResult? Function( ValueType record)?  appendOnly,}) {final _that = this;
switch (_that) {
case Merge_Lww() when lww != null:
return lww(_that.fields,_that.container);case Merge_AddWinsSet() when addWinsSet != null:
return addWinsSet(_that.element);case Merge_AppendOnly() when appendOnly != null:
return appendOnly(_that.record);case _:
  return null;

}
}

}

/// @nodoc


class Merge_Lww extends Merge {
  const Merge_Lww({required final  List<FieldDef> fields, this.container}): _fields = fields,super._();
  

 final  List<FieldDef> _fields;
 List<FieldDef> get fields {
  if (_fields is EqualUnmodifiableListView) return _fields;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_fields);
}

 final  ContainerDef? container;

/// Create a copy of Merge
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Merge_LwwCopyWith<Merge_Lww> get copyWith => _$Merge_LwwCopyWithImpl<Merge_Lww>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Merge_Lww&&const DeepCollectionEquality().equals(other._fields, _fields)&&(identical(other.container, container) || other.container == container));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(_fields),container);

@override
String toString() {
  return 'Merge.lww(fields: $fields, container: $container)';
}


}

/// @nodoc
abstract mixin class $Merge_LwwCopyWith<$Res> implements $MergeCopyWith<$Res> {
  factory $Merge_LwwCopyWith(Merge_Lww value, $Res Function(Merge_Lww) _then) = _$Merge_LwwCopyWithImpl;
@useResult
$Res call({
 List<FieldDef> fields, ContainerDef? container
});




}
/// @nodoc
class _$Merge_LwwCopyWithImpl<$Res>
    implements $Merge_LwwCopyWith<$Res> {
  _$Merge_LwwCopyWithImpl(this._self, this._then);

  final Merge_Lww _self;
  final $Res Function(Merge_Lww) _then;

/// Create a copy of Merge
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? fields = null,Object? container = freezed,}) {
  return _then(Merge_Lww(
fields: null == fields ? _self._fields : fields // ignore: cast_nullable_to_non_nullable
as List<FieldDef>,container: freezed == container ? _self.container : container // ignore: cast_nullable_to_non_nullable
as ContainerDef?,
  ));
}


}

/// @nodoc


class Merge_AddWinsSet extends Merge {
  const Merge_AddWinsSet({required this.element}): super._();
  

 final  ValueType element;

/// Create a copy of Merge
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Merge_AddWinsSetCopyWith<Merge_AddWinsSet> get copyWith => _$Merge_AddWinsSetCopyWithImpl<Merge_AddWinsSet>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Merge_AddWinsSet&&(identical(other.element, element) || other.element == element));
}


@override
int get hashCode => Object.hash(runtimeType,element);

@override
String toString() {
  return 'Merge.addWinsSet(element: $element)';
}


}

/// @nodoc
abstract mixin class $Merge_AddWinsSetCopyWith<$Res> implements $MergeCopyWith<$Res> {
  factory $Merge_AddWinsSetCopyWith(Merge_AddWinsSet value, $Res Function(Merge_AddWinsSet) _then) = _$Merge_AddWinsSetCopyWithImpl;
@useResult
$Res call({
 ValueType element
});




}
/// @nodoc
class _$Merge_AddWinsSetCopyWithImpl<$Res>
    implements $Merge_AddWinsSetCopyWith<$Res> {
  _$Merge_AddWinsSetCopyWithImpl(this._self, this._then);

  final Merge_AddWinsSet _self;
  final $Res Function(Merge_AddWinsSet) _then;

/// Create a copy of Merge
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? element = null,}) {
  return _then(Merge_AddWinsSet(
element: null == element ? _self.element : element // ignore: cast_nullable_to_non_nullable
as ValueType,
  ));
}


}

/// @nodoc


class Merge_AppendOnly extends Merge {
  const Merge_AppendOnly({required this.record}): super._();
  

 final  ValueType record;

/// Create a copy of Merge
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Merge_AppendOnlyCopyWith<Merge_AppendOnly> get copyWith => _$Merge_AppendOnlyCopyWithImpl<Merge_AppendOnly>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Merge_AppendOnly&&(identical(other.record, record) || other.record == record));
}


@override
int get hashCode => Object.hash(runtimeType,record);

@override
String toString() {
  return 'Merge.appendOnly(record: $record)';
}


}

/// @nodoc
abstract mixin class $Merge_AppendOnlyCopyWith<$Res> implements $MergeCopyWith<$Res> {
  factory $Merge_AppendOnlyCopyWith(Merge_AppendOnly value, $Res Function(Merge_AppendOnly) _then) = _$Merge_AppendOnlyCopyWithImpl;
@useResult
$Res call({
 ValueType record
});




}
/// @nodoc
class _$Merge_AppendOnlyCopyWithImpl<$Res>
    implements $Merge_AppendOnlyCopyWith<$Res> {
  _$Merge_AppendOnlyCopyWithImpl(this._self, this._then);

  final Merge_AppendOnly _self;
  final $Res Function(Merge_AppendOnly) _then;

/// Create a copy of Merge
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? record = null,}) {
  return _then(Merge_AppendOnly(
record: null == record ? _self.record : record // ignore: cast_nullable_to_non_nullable
as ValueType,
  ));
}


}

/// @nodoc
mixin _$Step {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Step);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'Step()';
}


}

/// @nodoc
class $StepCopyWith<$Res>  {
$StepCopyWith(Step _, $Res Function(Step) __);
}


/// Adds pattern-matching-related methods to [Step].
extension StepPatterns on Step {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Step_Sign value)?  sign,TResult Function( Step_Done value)?  done,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Step_Sign() when sign != null:
return sign(_that);case Step_Done() when done != null:
return done(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Step_Sign value)  sign,required TResult Function( Step_Done value)  done,}){
final _that = this;
switch (_that) {
case Step_Sign():
return sign(_that);case Step_Done():
return done(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Step_Sign value)?  sign,TResult? Function( Step_Done value)?  done,}){
final _that = this;
switch (_that) {
case Step_Sign() when sign != null:
return sign(_that);case Step_Done() when done != null:
return done(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( Uint8List signable)?  sign,TResult Function( Outcome field0)?  done,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Step_Sign() when sign != null:
return sign(_that.signable);case Step_Done() when done != null:
return done(_that.field0);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( Uint8List signable)  sign,required TResult Function( Outcome field0)  done,}) {final _that = this;
switch (_that) {
case Step_Sign():
return sign(_that.signable);case Step_Done():
return done(_that.field0);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( Uint8List signable)?  sign,TResult? Function( Outcome field0)?  done,}) {final _that = this;
switch (_that) {
case Step_Sign() when sign != null:
return sign(_that.signable);case Step_Done() when done != null:
return done(_that.field0);case _:
  return null;

}
}

}

/// @nodoc


class Step_Sign extends Step {
  const Step_Sign({required this.signable}): super._();
  

 final  Uint8List signable;

/// Create a copy of Step
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Step_SignCopyWith<Step_Sign> get copyWith => _$Step_SignCopyWithImpl<Step_Sign>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Step_Sign&&const DeepCollectionEquality().equals(other.signable, signable));
}


@override
int get hashCode => Object.hash(runtimeType,const DeepCollectionEquality().hash(signable));

@override
String toString() {
  return 'Step.sign(signable: $signable)';
}


}

/// @nodoc
abstract mixin class $Step_SignCopyWith<$Res> implements $StepCopyWith<$Res> {
  factory $Step_SignCopyWith(Step_Sign value, $Res Function(Step_Sign) _then) = _$Step_SignCopyWithImpl;
@useResult
$Res call({
 Uint8List signable
});




}
/// @nodoc
class _$Step_SignCopyWithImpl<$Res>
    implements $Step_SignCopyWith<$Res> {
  _$Step_SignCopyWithImpl(this._self, this._then);

  final Step_Sign _self;
  final $Res Function(Step_Sign) _then;

/// Create a copy of Step
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? signable = null,}) {
  return _then(Step_Sign(
signable: null == signable ? _self.signable : signable // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class Step_Done extends Step {
  const Step_Done(this.field0): super._();
  

 final  Outcome field0;

/// Create a copy of Step
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Step_DoneCopyWith<Step_Done> get copyWith => _$Step_DoneCopyWithImpl<Step_Done>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Step_Done&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode => Object.hash(runtimeType,field0);

@override
String toString() {
  return 'Step.done(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Step_DoneCopyWith<$Res> implements $StepCopyWith<$Res> {
  factory $Step_DoneCopyWith(Step_Done value, $Res Function(Step_Done) _then) = _$Step_DoneCopyWithImpl;
@useResult
$Res call({
 Outcome field0
});




}
/// @nodoc
class _$Step_DoneCopyWithImpl<$Res>
    implements $Step_DoneCopyWith<$Res> {
  _$Step_DoneCopyWithImpl(this._self, this._then);

  final Step_Done _self;
  final $Res Function(Step_Done) _then;

/// Create a copy of Step
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Step_Done(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Outcome,
  ));
}


}

/// @nodoc
mixin _$Write {





@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Write);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
  return 'Write()';
}


}

/// @nodoc
class $WriteCopyWith<$Res>  {
$WriteCopyWith(Write _, $Res Function(Write) __);
}


/// Adds pattern-matching-related methods to [Write].
extension WritePatterns on Write {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Write_Put value)?  put,TResult Function( Write_Delete value)?  delete,TResult Function( Write_Restore value)?  restore,TResult Function( Write_SetAdd value)?  setAdd,TResult Function( Write_SetRemove value)?  setRemove,TResult Function( Write_Append value)?  append,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Write_Put() when put != null:
return put(_that);case Write_Delete() when delete != null:
return delete(_that);case Write_Restore() when restore != null:
return restore(_that);case Write_SetAdd() when setAdd != null:
return setAdd(_that);case Write_SetRemove() when setRemove != null:
return setRemove(_that);case Write_Append() when append != null:
return append(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Write_Put value)  put,required TResult Function( Write_Delete value)  delete,required TResult Function( Write_Restore value)  restore,required TResult Function( Write_SetAdd value)  setAdd,required TResult Function( Write_SetRemove value)  setRemove,required TResult Function( Write_Append value)  append,}){
final _that = this;
switch (_that) {
case Write_Put():
return put(_that);case Write_Delete():
return delete(_that);case Write_Restore():
return restore(_that);case Write_SetAdd():
return setAdd(_that);case Write_SetRemove():
return setRemove(_that);case Write_Append():
return append(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Write_Put value)?  put,TResult? Function( Write_Delete value)?  delete,TResult? Function( Write_Restore value)?  restore,TResult? Function( Write_SetAdd value)?  setAdd,TResult? Function( Write_SetRemove value)?  setRemove,TResult? Function( Write_Append value)?  append,}){
final _that = this;
switch (_that) {
case Write_Put() when put != null:
return put(_that);case Write_Delete() when delete != null:
return delete(_that);case Write_Restore() when restore != null:
return restore(_that);case Write_SetAdd() when setAdd != null:
return setAdd(_that);case Write_SetRemove() when setRemove != null:
return setRemove(_that);case Write_Append() when append != null:
return append(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String table,  String row,  List<Field> fields)?  put,TResult Function( String table,  String row)?  delete,TResult Function( String table,  String row)?  restore,TResult Function( String set_,  KernelValue element)?  setAdd,TResult Function( String set_,  KernelValue element)?  setRemove,TResult Function( String stream,  KernelValue record)?  append,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Write_Put() when put != null:
return put(_that.table,_that.row,_that.fields);case Write_Delete() when delete != null:
return delete(_that.table,_that.row);case Write_Restore() when restore != null:
return restore(_that.table,_that.row);case Write_SetAdd() when setAdd != null:
return setAdd(_that.set_,_that.element);case Write_SetRemove() when setRemove != null:
return setRemove(_that.set_,_that.element);case Write_Append() when append != null:
return append(_that.stream,_that.record);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String table,  String row,  List<Field> fields)  put,required TResult Function( String table,  String row)  delete,required TResult Function( String table,  String row)  restore,required TResult Function( String set_,  KernelValue element)  setAdd,required TResult Function( String set_,  KernelValue element)  setRemove,required TResult Function( String stream,  KernelValue record)  append,}) {final _that = this;
switch (_that) {
case Write_Put():
return put(_that.table,_that.row,_that.fields);case Write_Delete():
return delete(_that.table,_that.row);case Write_Restore():
return restore(_that.table,_that.row);case Write_SetAdd():
return setAdd(_that.set_,_that.element);case Write_SetRemove():
return setRemove(_that.set_,_that.element);case Write_Append():
return append(_that.stream,_that.record);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String table,  String row,  List<Field> fields)?  put,TResult? Function( String table,  String row)?  delete,TResult? Function( String table,  String row)?  restore,TResult? Function( String set_,  KernelValue element)?  setAdd,TResult? Function( String set_,  KernelValue element)?  setRemove,TResult? Function( String stream,  KernelValue record)?  append,}) {final _that = this;
switch (_that) {
case Write_Put() when put != null:
return put(_that.table,_that.row,_that.fields);case Write_Delete() when delete != null:
return delete(_that.table,_that.row);case Write_Restore() when restore != null:
return restore(_that.table,_that.row);case Write_SetAdd() when setAdd != null:
return setAdd(_that.set_,_that.element);case Write_SetRemove() when setRemove != null:
return setRemove(_that.set_,_that.element);case Write_Append() when append != null:
return append(_that.stream,_that.record);case _:
  return null;

}
}

}

/// @nodoc


class Write_Put extends Write {
  const Write_Put({required this.table, required this.row, required final  List<Field> fields}): _fields = fields,super._();
  

 final  String table;
 final  String row;
 final  List<Field> _fields;
 List<Field> get fields {
  if (_fields is EqualUnmodifiableListView) return _fields;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_fields);
}


/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Write_PutCopyWith<Write_Put> get copyWith => _$Write_PutCopyWithImpl<Write_Put>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Write_Put&&(identical(other.table, table) || other.table == table)&&(identical(other.row, row) || other.row == row)&&const DeepCollectionEquality().equals(other._fields, _fields));
}


@override
int get hashCode => Object.hash(runtimeType,table,row,const DeepCollectionEquality().hash(_fields));

@override
String toString() {
  return 'Write.put(table: $table, row: $row, fields: $fields)';
}


}

/// @nodoc
abstract mixin class $Write_PutCopyWith<$Res> implements $WriteCopyWith<$Res> {
  factory $Write_PutCopyWith(Write_Put value, $Res Function(Write_Put) _then) = _$Write_PutCopyWithImpl;
@useResult
$Res call({
 String table, String row, List<Field> fields
});




}
/// @nodoc
class _$Write_PutCopyWithImpl<$Res>
    implements $Write_PutCopyWith<$Res> {
  _$Write_PutCopyWithImpl(this._self, this._then);

  final Write_Put _self;
  final $Res Function(Write_Put) _then;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? table = null,Object? row = null,Object? fields = null,}) {
  return _then(Write_Put(
table: null == table ? _self.table : table // ignore: cast_nullable_to_non_nullable
as String,row: null == row ? _self.row : row // ignore: cast_nullable_to_non_nullable
as String,fields: null == fields ? _self._fields : fields // ignore: cast_nullable_to_non_nullable
as List<Field>,
  ));
}


}

/// @nodoc


class Write_Delete extends Write {
  const Write_Delete({required this.table, required this.row}): super._();
  

 final  String table;
 final  String row;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Write_DeleteCopyWith<Write_Delete> get copyWith => _$Write_DeleteCopyWithImpl<Write_Delete>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Write_Delete&&(identical(other.table, table) || other.table == table)&&(identical(other.row, row) || other.row == row));
}


@override
int get hashCode => Object.hash(runtimeType,table,row);

@override
String toString() {
  return 'Write.delete(table: $table, row: $row)';
}


}

/// @nodoc
abstract mixin class $Write_DeleteCopyWith<$Res> implements $WriteCopyWith<$Res> {
  factory $Write_DeleteCopyWith(Write_Delete value, $Res Function(Write_Delete) _then) = _$Write_DeleteCopyWithImpl;
@useResult
$Res call({
 String table, String row
});




}
/// @nodoc
class _$Write_DeleteCopyWithImpl<$Res>
    implements $Write_DeleteCopyWith<$Res> {
  _$Write_DeleteCopyWithImpl(this._self, this._then);

  final Write_Delete _self;
  final $Res Function(Write_Delete) _then;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? table = null,Object? row = null,}) {
  return _then(Write_Delete(
table: null == table ? _self.table : table // ignore: cast_nullable_to_non_nullable
as String,row: null == row ? _self.row : row // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class Write_Restore extends Write {
  const Write_Restore({required this.table, required this.row}): super._();
  

 final  String table;
 final  String row;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Write_RestoreCopyWith<Write_Restore> get copyWith => _$Write_RestoreCopyWithImpl<Write_Restore>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Write_Restore&&(identical(other.table, table) || other.table == table)&&(identical(other.row, row) || other.row == row));
}


@override
int get hashCode => Object.hash(runtimeType,table,row);

@override
String toString() {
  return 'Write.restore(table: $table, row: $row)';
}


}

/// @nodoc
abstract mixin class $Write_RestoreCopyWith<$Res> implements $WriteCopyWith<$Res> {
  factory $Write_RestoreCopyWith(Write_Restore value, $Res Function(Write_Restore) _then) = _$Write_RestoreCopyWithImpl;
@useResult
$Res call({
 String table, String row
});




}
/// @nodoc
class _$Write_RestoreCopyWithImpl<$Res>
    implements $Write_RestoreCopyWith<$Res> {
  _$Write_RestoreCopyWithImpl(this._self, this._then);

  final Write_Restore _self;
  final $Res Function(Write_Restore) _then;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? table = null,Object? row = null,}) {
  return _then(Write_Restore(
table: null == table ? _self.table : table // ignore: cast_nullable_to_non_nullable
as String,row: null == row ? _self.row : row // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class Write_SetAdd extends Write {
  const Write_SetAdd({required this.set_, required this.element}): super._();
  

 final  String set_;
 final  KernelValue element;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Write_SetAddCopyWith<Write_SetAdd> get copyWith => _$Write_SetAddCopyWithImpl<Write_SetAdd>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Write_SetAdd&&(identical(other.set_, set_) || other.set_ == set_)&&(identical(other.element, element) || other.element == element));
}


@override
int get hashCode => Object.hash(runtimeType,set_,element);

@override
String toString() {
  return 'Write.setAdd(set_: $set_, element: $element)';
}


}

/// @nodoc
abstract mixin class $Write_SetAddCopyWith<$Res> implements $WriteCopyWith<$Res> {
  factory $Write_SetAddCopyWith(Write_SetAdd value, $Res Function(Write_SetAdd) _then) = _$Write_SetAddCopyWithImpl;
@useResult
$Res call({
 String set_, KernelValue element
});


$KernelValueCopyWith<$Res> get element;

}
/// @nodoc
class _$Write_SetAddCopyWithImpl<$Res>
    implements $Write_SetAddCopyWith<$Res> {
  _$Write_SetAddCopyWithImpl(this._self, this._then);

  final Write_SetAdd _self;
  final $Res Function(Write_SetAdd) _then;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? set_ = null,Object? element = null,}) {
  return _then(Write_SetAdd(
set_: null == set_ ? _self.set_ : set_ // ignore: cast_nullable_to_non_nullable
as String,element: null == element ? _self.element : element // ignore: cast_nullable_to_non_nullable
as KernelValue,
  ));
}

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$KernelValueCopyWith<$Res> get element {
  
  return $KernelValueCopyWith<$Res>(_self.element, (value) {
    return _then(_self.copyWith(element: value));
  });
}
}

/// @nodoc


class Write_SetRemove extends Write {
  const Write_SetRemove({required this.set_, required this.element}): super._();
  

 final  String set_;
 final  KernelValue element;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Write_SetRemoveCopyWith<Write_SetRemove> get copyWith => _$Write_SetRemoveCopyWithImpl<Write_SetRemove>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Write_SetRemove&&(identical(other.set_, set_) || other.set_ == set_)&&(identical(other.element, element) || other.element == element));
}


@override
int get hashCode => Object.hash(runtimeType,set_,element);

@override
String toString() {
  return 'Write.setRemove(set_: $set_, element: $element)';
}


}

/// @nodoc
abstract mixin class $Write_SetRemoveCopyWith<$Res> implements $WriteCopyWith<$Res> {
  factory $Write_SetRemoveCopyWith(Write_SetRemove value, $Res Function(Write_SetRemove) _then) = _$Write_SetRemoveCopyWithImpl;
@useResult
$Res call({
 String set_, KernelValue element
});


$KernelValueCopyWith<$Res> get element;

}
/// @nodoc
class _$Write_SetRemoveCopyWithImpl<$Res>
    implements $Write_SetRemoveCopyWith<$Res> {
  _$Write_SetRemoveCopyWithImpl(this._self, this._then);

  final Write_SetRemove _self;
  final $Res Function(Write_SetRemove) _then;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? set_ = null,Object? element = null,}) {
  return _then(Write_SetRemove(
set_: null == set_ ? _self.set_ : set_ // ignore: cast_nullable_to_non_nullable
as String,element: null == element ? _self.element : element // ignore: cast_nullable_to_non_nullable
as KernelValue,
  ));
}

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$KernelValueCopyWith<$Res> get element {
  
  return $KernelValueCopyWith<$Res>(_self.element, (value) {
    return _then(_self.copyWith(element: value));
  });
}
}

/// @nodoc


class Write_Append extends Write {
  const Write_Append({required this.stream, required this.record}): super._();
  

 final  String stream;
 final  KernelValue record;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Write_AppendCopyWith<Write_Append> get copyWith => _$Write_AppendCopyWithImpl<Write_Append>(this, _$identity);



@override
bool operator ==(Object other) {
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Write_Append&&(identical(other.stream, stream) || other.stream == stream)&&(identical(other.record, record) || other.record == record));
}


@override
int get hashCode => Object.hash(runtimeType,stream,record);

@override
String toString() {
  return 'Write.append(stream: $stream, record: $record)';
}


}

/// @nodoc
abstract mixin class $Write_AppendCopyWith<$Res> implements $WriteCopyWith<$Res> {
  factory $Write_AppendCopyWith(Write_Append value, $Res Function(Write_Append) _then) = _$Write_AppendCopyWithImpl;
@useResult
$Res call({
 String stream, KernelValue record
});


$KernelValueCopyWith<$Res> get record;

}
/// @nodoc
class _$Write_AppendCopyWithImpl<$Res>
    implements $Write_AppendCopyWith<$Res> {
  _$Write_AppendCopyWithImpl(this._self, this._then);

  final Write_Append _self;
  final $Res Function(Write_Append) _then;

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? stream = null,Object? record = null,}) {
  return _then(Write_Append(
stream: null == stream ? _self.stream : stream // ignore: cast_nullable_to_non_nullable
as String,record: null == record ? _self.record : record // ignore: cast_nullable_to_non_nullable
as KernelValue,
  ));
}

/// Create a copy of Write
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$KernelValueCopyWith<$Res> get record {
  
  return $KernelValueCopyWith<$Res>(_self.record, (value) {
    return _then(_self.copyWith(record: value));
  });
}
}

// dart format on
